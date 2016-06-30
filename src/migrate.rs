extern crate git2;

use git2::*;
use std::path::Path;
use scratch::Scratch;


pub enum ReviewUploadResult
{
    Uploaded(Oid),
    RejectNoFF,
    RejectMerge,
    NoChanges,
    Central,
}

pub fn review_upload(scratch: &Scratch, newrev: Object, module: &str) -> ReviewUploadResult
{
    debug!(".\n\n==== Doing review upload for module {}", &module);

    let new = newrev.id();
    let old = scratch.tracking(&module, "master").expect("no tracking branch 1").id();

    if old == new {
        return ReviewUploadResult::NoChanges;
    }

    match scratch.repo.graph_descendant_of(new, old) {
        Err(_) => return ReviewUploadResult::RejectNoFF,
        Ok(false) => return ReviewUploadResult::RejectNoFF,
        Ok(true) => (),
    }

    debug!("==== walking commits from {} to {}", old, new);

    let walk = {
        let mut walk = scratch.repo.revwalk().expect("walk: can't create revwalk");
        walk.set_sorting(SORT_REVERSE | SORT_TIME);
        let range = format!("{}..{}", old, new);
        walk.push_range(&range).expect(&format!("walk: invalid range: {}", range));;
        walk
    };

    let mut current =
        scratch.tracking(scratch.host.central(), "master").expect("no central tracking").id();

    for rev in walk {
        let rev = rev.expect("walk: invalid rev");
        if old == rev {
            continue;
        }

        debug!("==== walking commit {}", rev);

        let module_commit = scratch.repo
            .find_commit(rev)
            .expect("walk: object is not actually a commit");

        if module_commit.parents().count() > 1 {
            // TODO: also do this check on pushes to cenral refs/for/master
            // TODO: invectigate the possibility of allowing merge commits
            return ReviewUploadResult::RejectMerge;
        }

        if module != scratch.host.central() {
            debug!("==== Rewriting commit {}", rev);

            let tree = module_commit.tree().expect("walk: commit has no tree");
            let parent =
                scratch.repo.find_commit(current).expect("walk: current object is no commit");

            let new_tree = scratch.replace_subtree(Path::new(module),
                                                   tree.id(),
                                                   parent.tree()
                                                       .expect("walk: parent has no tree"));

            current = scratch.rewrite(&module_commit, &vec![&parent], &new_tree);
        }
    }


    if module != scratch.host.central() {
        return ReviewUploadResult::Uploaded(current);
    }
    else {
        return ReviewUploadResult::Central;
    }
}

pub fn project_created(scratch: &Scratch)
{
    if let Some(rev) = scratch.tracking(scratch.host.central(), "master") {
        central_submit(scratch, rev);
    }
}

pub fn central_submit(scratch: &Scratch, newrev: Object)
{
    debug!(" ---> central_submit (sha1 of commit: {})", &newrev.id());

    let central_commit = newrev.as_commit().expect("could not get commit from obj");
    let central_tree = central_commit.tree().expect("commit has no tree");

    for module in scratch.host.projects() {
        if module == scratch.host.central() {
            continue;
        }
        debug!("");
        debug!("==== fetching tracking branch for module: {}", &module);
        let module_master_commit_obj = match scratch.tracking(&module, "master") {
            Some(obj) => obj,
            None => {
                debug!("====    no tracking branch for module {} => project does not exist or is \
                        empty",
                       &module);
                debug!("====    initializing with subdir history");
                let commit = scratch.split_subdir(&module, newrev.id());
                scratch.push(commit.id(), &module, "refs/heads/master");
                scratch.tracking(&module, "master")
                    .expect(&format!("no tracking branch for module: {}", module))
            }
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
            let output = scratch.push(module_commit, &module, "master");
            debug!("{}", output);
        }
        else {
            debug!("====    commit does not change module => skipping");
        }
    }
}
