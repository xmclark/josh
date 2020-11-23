fn baseref_and_options(
    refname: &str,
) -> josh::JoshResult<(String, String, Vec<String>)> {
    let mut split = refname.splitn(2, '%');
    let push_to = split.next().ok_or(josh::josh_error("no next"))?.to_owned();

    let options = if let Some(options) = split.next() {
        options.split(',').map(|x| x.to_string()).collect()
    } else {
        vec![]
    };

    let mut baseref = push_to.to_owned();

    if baseref.starts_with("refs/for") {
        baseref = baseref.replacen("refs/for", "refs/heads", 1)
    }
    if baseref.starts_with("refs/drafts") {
        baseref = baseref.replacen("refs/drafts", "refs/heads", 1)
    }
    return Ok((baseref, push_to, options));
}

pub fn process_repo_update(
    repo_update: std::collections::HashMap<String, String>,
    _forward_maps: std::sync::Arc<std::sync::RwLock<josh::view_maps::ViewMaps>>,
    backward_maps: std::sync::Arc<std::sync::RwLock<josh::view_maps::ViewMaps>>,
) -> Result<String, josh::JoshError> {
    let ru = {
        let mut ru = repo_update.clone();
        ru.insert("password".to_owned(), "...".to_owned());
    };
    let _trace_s = tracing::span!(tracing::Level::TRACE, "process_repo_update", repo_update= ?ru);
    let refname = repo_update.get("refname").ok_or(josh::josh_error(""))?;
    let filter_spec =
        repo_update.get("filter_spec").ok_or(josh::josh_error(""))?;
    let old = repo_update.get("old").ok_or(josh::josh_error(""))?;
    let new = repo_update.get("new").ok_or(josh::josh_error(""))?;
    let username = repo_update.get("username").ok_or(josh::josh_error(""))?;
    let password = repo_update.get("password").ok_or(josh::josh_error(""))?;
    let remote_url =
        repo_update.get("remote_url").ok_or(josh::josh_error(""))?;
    let base_ns = repo_update.get("base_ns").ok_or(josh::josh_error(""))?;
    let git_dir = repo_update.get("GIT_DIR").ok_or(josh::josh_error(""))?;
    let git_ns = repo_update
        .get("GIT_NAMESPACE")
        .ok_or(josh::josh_error(""))?;
    tracing::debug!("REPO_UPDATE env ok");

    let repo = git2::Repository::init_bare(&std::path::Path::new(&git_dir))?;

    let old = git2::Oid::from_str(old)?;

    let (baseref, push_to, options) = baseref_and_options(refname)?;
    let josh_merge = options.contains(&"josh-merge".to_string());

    tracing::debug!("push options: {:?}", options);
    tracing::debug!("josh-merge: {:?}", josh_merge);

    let old = if old == git2::Oid::zero() {
        let rev = format!("refs/namespaces/{}/{}", git_ns, &baseref);
        let oid = if let Ok(x) = repo.revparse_single(&rev) {
            x.id()
        } else {
            old
        };
        tracing::trace!("push: old oid: {:?}, rev: {:?}", oid, rev);
        oid
    } else {
        tracing::trace!("push: old oid: {:?}, refname: {:?}", old, refname);
        old
    };

    let viewobj = josh::filters::parse(&filter_spec);
    let new_oid = git2::Oid::from_str(&new)?;
    let backward_new_oid = {
        tracing::debug!("=== MORE");

        tracing::debug!("=== processed_old {:?}", old);

        match josh::scratch::unapply_view(
            &repo,
            backward_maps,
            &*viewobj,
            old,
            new_oid,
        )? {
            josh::UnapplyView::Done(rewritten) => {
                tracing::debug!("rewritten");
                rewritten
            }
            josh::UnapplyView::BranchDoesNotExist => {
                return Err(josh::josh_error(
                    "branch does not exist on remote",
                ));
            }
            josh::UnapplyView::RejectMerge(parent_count) => {
                return Err(josh::josh_error(&format!(
                    "rejecting merge with {} parents",
                    parent_count
                )));
            }
        }
    };

    let oid_to_push = if josh_merge {
        let rev = format!("refs/josh/upstream/{}/{}", &base_ns, &baseref);
        let backward_commit = repo.find_commit(backward_new_oid)?;
        if let Ok(Ok(base_commit)) =
            repo.revparse_single(&rev).map(|x| x.peel_to_commit())
        {
            let merged_tree = repo
                .merge_commits(&base_commit, &backward_commit, None)?
                .write_tree_to(&repo)?;
            repo.commit(
                None,
                &backward_commit.author(),
                &backward_commit.committer(),
                &format!("Merge from {}", &filter_spec),
                &repo.find_tree(merged_tree)?,
                &[&base_commit, &backward_commit],
            )?
        } else {
            return Err(josh::josh_error("josh_merge failed"));
        }
    } else {
        backward_new_oid
    };

    let mut options = options;
    options.retain(|x| !x.starts_with("josh-"));
    let options = options;

    let push_with_options = if options.len() != 0 {
        push_to + "%" + &options.join(",")
    } else {
        push_to
    };

    return push_head_url(
        &repo,
        oid_to_push,
        &push_with_options,
        &remote_url,
        &username,
        &password,
        &git_ns,
    );
}

fn push_head_url(
    repo: &git2::Repository,
    oid: git2::Oid,
    refname: &str,
    url: &str,
    username: &str,
    password: &str,
    namespace: &str,
) -> josh::JoshResult<String> {
    let rn = format!("refs/{}", &namespace);

    let spec = format!("{}:{}", &rn, &refname);

    let shell = josh::shell::Shell {
        cwd: repo.path().to_owned(),
    };
    let nurl = if username != "" {
        let splitted: Vec<&str> = url.splitn(2, "://").collect();
        let proto = splitted[0];
        let rest = splitted[1];
        format!("{}://{}@{}", &proto, &username, &rest)
    } else {
        url.to_owned()
    };
    let cmd = format!("git push {} '{}'", &nurl, &spec);
    let mut fakehead = repo.reference(&rn, oid, true, "push_head_url")?;
    let (stdout, stderr) =
        shell.command_env(&cmd, &[("GIT_PASSWORD", &password)]);
    fakehead.delete()?;
    tracing::debug!("{}", &stderr);
    tracing::debug!("{}", &stdout);

    let stderr = stderr.replace(&rn, "JOSH_PUSH");

    return Ok(stderr);
}

pub fn create_repo(path: &std::path::Path) -> josh::JoshResult<()> {
    tracing::debug!("init base repo: {:?}", path);
    std::fs::create_dir_all(path).expect("can't create_dir_all");
    git2::Repository::init_bare(path)?;
    let shell = josh::shell::Shell {
        cwd: path.to_path_buf(),
    };
    shell.command("git config http.receivepack true");
    let ce = std::env::current_exe().expect("can't find path to exe");
    shell.command("rm -Rf hooks");
    shell.command("mkdir hooks");
    std::os::unix::fs::symlink(ce, path.join("hooks").join("update"))
        .expect("can't symlink update hook");
    shell.command(&format!(
        "git config credential.helper '!f() {{ echo \"password=\"$GIT_PASSWORD\"\"; }}; f'"
    ));
    shell.command(&"git config gc.auto 0");

    if std::env::var_os("JOSH_KEEP_NS") == None {
        std::fs::remove_dir_all(path.join("refs/namespaces")).ok();
    }
    tracing::info!("repo initialized");
    return Ok(());
}

pub fn fetch_refs_from_url(
    path: &std::path::Path,
    upstream_repo: &str,
    url: &str,
    refs_prefixes: &[&str],
    username: &str,
    password: &Password,
) -> Result<(), git2::Error> {
    let specs: Vec<_> = refs_prefixes
        .iter()
        .map(|r| {
            format!(
                "'+{}:refs/josh/upstream/{}/{}'",
                &r,
                josh::to_ns(upstream_repo),
                &r
            )
        })
        .collect();

    let shell = josh::shell::Shell {
        cwd: path.to_owned(),
    };
    let nurl = if username != "" {
        let splitted: Vec<&str> = url.splitn(2, "://").collect();
        let proto = splitted[0];
        let rest = splitted[1];
        format!("{}://{}@{}", &proto, &username, &rest)
    } else {
        let splitted: Vec<&str> = url.splitn(2, "://").collect();
        let proto = splitted[0];
        let rest = splitted[1];
        format!("{}://{}@{}", &proto, "annonymous", &rest)
    };

    let cmd = format!("git fetch --no-tags {} {}", &nurl, &specs.join(" "));
    tracing::info!("fetch_refs_from_url {:?} {:?} {:?}", cmd, path, "");

    let (_stdout, stderr) =
        shell.command_env(&cmd, &[("GIT_PASSWORD", &password.value)]);
    tracing::debug!(
        "fetch_refs_from_url done {:?} {:?} {:?}",
        cmd,
        path,
        stderr
    );
    if stderr.contains("fatal: Authentication failed") {
        return Err(git2::Error::from_str("auth"));
    }
    if stderr.contains("fatal:") {
        return Err(git2::Error::from_str("error"));
    }
    if stderr.contains("error:") {
        return Err(git2::Error::from_str("error"));
    }
    return Ok(());
}

// Wrapper struct for storing passwords to avoid having
// them output to traces by accident
#[derive(Clone)]
pub struct Password {
    pub value: String,
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Password")
            .field("value", &"<hidden>")
            .finish()
    }
}

pub struct TmpGitNamespace {
    name: String,
    repo_path: std::path::PathBuf,
}

impl TmpGitNamespace {
    pub fn new(repo_path: &std::path::Path) -> TmpGitNamespace {
        TmpGitNamespace {
            name: format!("request_{}", uuid::Uuid::new_v4()),
            repo_path: repo_path.to_owned(),
        }
    }

    pub fn name(&self) -> &str {
        return &self.name;
    }
    pub fn reference(&self, refname: &str) -> String {
        return format!("refs/namespaces/{}/{}", &self.name, refname);
    }
}

impl std::fmt::Debug for TmpGitNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoshProxyService")
            .field("repo_path", &self.repo_path)
            .field("name", &self.name)
            .finish()
    }
}

impl Drop for TmpGitNamespace {
    fn drop(&mut self) {
        if std::env::var_os("JOSH_KEEP_NS") != None {
            return;
        }
        let request_tmp_namespace =
            self.repo_path.join("refs/namespaces").join(&self.name);
        std::fs::remove_dir_all(request_tmp_namespace).unwrap_or_else(|e| {
            tracing::warn!("remove_dir_all failed: {:?}", e)
        });
    }
}
