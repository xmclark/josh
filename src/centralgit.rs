extern crate git2;

use git2::*;
use std::path::Path;
use scratch::Scratch;
use super::Hooks;
use super::RepoHost;
use super::ReviewUploadResult;

pub struct CentralGit;

pub fn module_ref(module: &str) -> String
{
    format!("refs/centralgit/{}/##master##", module)
}



impl Hooks for CentralGit
{
    fn review_upload(&self,
                     scratch: &Scratch,
                     host: &RepoHost,
                     newrev: Object,
                     module: &str)
        -> ReviewUploadResult
    {
        debug!(".\n\n==== Doing review upload for module {}", &module);

        let new = newrev.id();

        let (walk, initial) = if let Some(old) = scratch.tracking(host, &module, "master") {

            let old = old.id();

            if old == new {
                return ReviewUploadResult::NoChanges;
            }

            match scratch.repo.graph_descendant_of(new, old) {
                Err(_) | Ok(false) => return ReviewUploadResult::RejectNoFF,
                Ok(true) => (),
            }

            debug!("==== walking commits from {} to {}", old, new);

            let mut walk = scratch.repo.revwalk().expect("walk: can't create revwalk");
            walk.set_sorting(SORT_REVERSE | SORT_TOPOLOGICAL);
            let range = format!("{}..{}", old, new);
            walk.push_range(&range).expect(&format!("walk: invalid range: {}", range));;
            walk.hide(old).expect("walk: can't hide");
            (walk, false)
        }
        else {
            let mut walk = scratch.repo.revwalk().expect("walk: can't create revwalk");
            walk.set_sorting(SORT_REVERSE | SORT_TOPOLOGICAL);
            walk.push(new).expect("walk: can't push");
            (walk, true)
        };

        let mut current =
            scratch.tracking(host, host.central(), "master").expect("no central tracking").id();

        for rev in walk {
            let rev = rev.expect("walk: invalid rev");

            debug!("==== walking commit {}", rev);

            let module_commit = scratch.repo
                .find_commit(rev)
                .expect("walk: object is not actually a commit");

            if module_commit.parents().count() > 1 {
                // TODO: also do this check on pushes to cenral refs/for/master
                // TODO: invectigate the possibility of allowing merge commits
                return ReviewUploadResult::RejectMerge;
            }

            if module != host.central() {
                debug!("==== Rewriting commit {}", rev);

                let tree = module_commit.tree().expect("walk: commit has no tree");
                let parent =
                    scratch.repo.find_commit(current).expect("walk: current object is no commit");

                let new_tree = scratch.replace_subtree(Path::new(module),
                                                       tree.id(),
                                                       &parent.tree()
                                                           .expect("walk: parent has no tree"));

                current = scratch.rewrite(&module_commit, &vec![&parent], &new_tree);
            }
        }


        if module != host.central() {
            return ReviewUploadResult::Uploaded(current, initial);
        }
        else {
            return ReviewUploadResult::Central;
        }
    }

    fn pre_create_project(&self, scratch: &Scratch, rev: Oid, project: &str)
    {
        if let Ok(_) = scratch.repo.refname_to_id(&module_ref(project)) {}
        else {
            if let Some(commit) = scratch.split_subdir(&project, rev) {
                scratch.repo
                    .reference(&module_ref(project), commit, true, "subtree_split")
                    .expect("can't create reference");
            }
        }
    }

    fn project_created(&self, scratch: &Scratch, host: &RepoHost, _project: &str)
    {
        if let Some(rev) = scratch.tracking(host, host.central(), "master") {
            self.central_submit(scratch, host, rev);
        }
    }

    fn central_submit(&self, scratch: &Scratch, host: &RepoHost, newrev: Object)
    {
        debug!(" ---> central_submit (sha1 of commit: {})", &newrev.id());

        let central_commit = newrev.as_commit().expect("could not get commit from obj");
        let central_tree = central_commit.tree().expect("commit has no tree");

        for module in host.projects() {
            if module == host.central() {
                continue;
            }
            debug!("");
            debug!("==== fetching tracking branch for module: {}", &module);
            match scratch.tracking(host, &module, "master") {
                Some(_) => (),
                None => {
                    debug!("====    no tracking branch for module {} => project does not exist \
                            or is empty",
                           &module);
                    debug!("====    initializing with subdir history");

                    self.pre_create_project(scratch, newrev.id(), &module);
                }
            };

            let module_master_commit_obj = if let Ok(rev) = scratch.repo
                .revparse_single(&module_ref(&module)) {
                rev
            }
            else {
                continue;
            };

            let parents = vec![module_master_commit_obj.as_commit()
                                   .expect("could not get commit from obj")];

            debug!("==== checking for changes in module: {:?}", module);

            // new tree is sub-tree of complete central tree
            let old_tree_id = if let Ok(tree) = parents[0].tree() {
                tree.id()
            }
            else {
                Oid::from_str("0000000000000000000000000000000000000000").unwrap()
            };

            let new_tree_id = if let Ok(tree_entry) = central_tree.get_path(&Path::new(&module)) {
                tree_entry.id()
            }
            else {
                Oid::from_str("0000000000000000000000000000000000000000").unwrap()
            };


            // if sha1's are equal the content is equal
            if new_tree_id != old_tree_id && !new_tree_id.is_zero() {
                let new_tree =
                    scratch.repo.find_tree(new_tree_id).expect("central_submit: can't find tree");
                debug!("====    commit changes module => make commit on module");
                let module_commit = scratch.rewrite(central_commit, &parents, &new_tree);
                scratch.repo
                    .reference(&module_ref(&module), module_commit, true, "rewrite")
                    .expect("can't create reference");
            }
            else {
                debug!("====    commit does not change module => skipping");
            }
        }

        for module in host.projects() {
            if let Ok(module_commit) = scratch.repo.refname_to_id(&module_ref(&module)) {
                let output = scratch.push(host, module_commit, &module, "refs/heads/master");
                debug!("{}", output);
            }
        }
    }
}
