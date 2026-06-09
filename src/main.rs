use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, String>;

const DEFAULT_CODEX_CMD: &str = "codex --sandbox workspace-write --ask-for-approval on-request";
const DEFAULT_CLAUDE_CMD: &str = "claude";

#[derive(Debug, Clone)]
struct Paths {
    app_dir: PathBuf,
    repo_dir: PathBuf,
    account_dir: PathBuf,
    key_dir: PathBuf,
    code_dir: PathBuf,
    ssh_config: PathBuf,
    ssh_config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct RepoName(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct AccountName(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct GitHubRepoUrl(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct SshHostAlias(String);

impl RepoName {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl AccountName {
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RepoConfig {
    name: RepoName,
    project: PathBuf,
    git_url: GitHubRepoUrl,
    clone_url: String,
    account: Option<AccountName>,
    key_file: Option<PathBuf>,
    host_alias: Option<SshHostAlias>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountConfig {
    git_user_name: String,
    git_user_email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubRepo {
    owner: String,
    repo: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cli {
    Register {
        name: String,
        project: String,
        account: Option<String>,
    },
    Add {
        github_url: String,
    },
    Clone {
        name: String,
    },
    Key {
        name: String,
    },
    List,
    AccountAdd {
        name: String,
        git_user_name: String,
        git_user_email: String,
    },
    AccountList,
    Open {
        name: String,
        llm: Llm,
    },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Llm {
    Codex,
    Claude,
}

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("repo: {err}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<OsString>) -> Result<()> {
    let cli = parse_cli(args)?;
    if cli == Cli::Help {
        print_usage();
        return Ok(());
    }

    let paths = Paths::from_env()?;

    match cli {
        Cli::Register {
            name,
            project,
            account,
        } => cmd_register(&paths, &name, &project, account.as_deref()),
        Cli::Add { github_url } => cmd_add(&paths, &github_url),
        Cli::Clone { name } => cmd_clone(&paths, &name),
        Cli::Key { name } => cmd_key(&paths, &name),
        Cli::List => cmd_list(&paths),
        Cli::AccountAdd {
            name,
            git_user_name,
            git_user_email,
        } => cmd_account_add(&paths, &name, &git_user_name, &git_user_email),
        Cli::AccountList => cmd_account_list(&paths),
        Cli::Open { name, llm } => cmd_open(&paths, &name, llm),
        Cli::Help => unreachable!(),
    }
}

fn parse_cli(args: Vec<OsString>) -> Result<Cli> {
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_string())
        })
        .collect::<Result<_>>()?;

    let Some(cmd) = args.first().map(String::as_str) else {
        return Ok(Cli::Help);
    };

    match cmd {
        "-h" | "--help" | "help" => Ok(Cli::Help),
        "register" => match args.as_slice() {
            [_, name, project] => Ok(Cli::Register {
                name: name.clone(),
                project: project.clone(),
                account: None,
            }),
            [_, name, project, account] => Ok(Cli::Register {
                name: name.clone(),
                project: project.clone(),
                account: Some(account.clone()),
            }),
            _ => Err("usage: repo register NAME PROJECT_DIR [ACCOUNT]".to_string()),
        },
        "add" => match args.as_slice() {
            [_, github_url] => Ok(Cli::Add {
                github_url: github_url.clone(),
            }),
            _ => Err("usage: repo add GITHUB_URL".to_string()),
        },
        "clone" => match args.as_slice() {
            [_, name] => Ok(Cli::Clone { name: name.clone() }),
            _ => Err("usage: repo clone NAME".to_string()),
        },
        "key" => match args.as_slice() {
            [_, name] => Ok(Cli::Key { name: name.clone() }),
            _ => Err("usage: repo key NAME".to_string()),
        },
        "list" => match args.as_slice() {
            [_] => Ok(Cli::List),
            _ => Err("usage: repo list".to_string()),
        },
        "account" => match args.as_slice() {
            [_, subcmd, name, git_user_name, git_user_email] if subcmd == "add" => {
                Ok(Cli::AccountAdd {
                    name: name.clone(),
                    git_user_name: git_user_name.clone(),
                    git_user_email: git_user_email.clone(),
                })
            }
            [_, subcmd] if subcmd == "list" => Ok(Cli::AccountList),
            _ => Err("usage: repo account add NAME GIT_USER_NAME GIT_USER_EMAIL\n       repo account list"
                .to_string()),
        },
        "codex" | "claude" => match args.as_slice() {
            [llm, name] => Ok(Cli::Open {
                name: name.clone(),
                llm: parse_llm(llm)?,
            }),
            _ => Err(format!("usage: repo {cmd} NAME")),
        },
        "open" => match args.as_slice() {
            [_, name] => Ok(Cli::Open {
                name: name.clone(),
                llm: Llm::Codex,
            }),
            [_, name, llm] => Ok(Cli::Open {
                name: name.clone(),
                llm: parse_llm(llm)?,
            }),
            _ => Err("usage: repo open NAME [codex|claude]".to_string()),
        },
        _ => {
            print_usage();
            Err(format!("unknown command: {cmd}"))
        }
    }
}

fn parse_llm(value: &str) -> Result<Llm> {
    match value {
        "codex" => Ok(Llm::Codex),
        "claude" => Ok(Llm::Claude),
        _ => Err(format!("unknown llm: {value}")),
    }
}

fn print_usage() {
    println!(
        "\
usage:
  repo register NAME PROJECT_DIR [ACCOUNT]
  repo add GITHUB_URL
  repo clone NAME
  repo key NAME
  repo list

  repo account add NAME GIT_USER_NAME GIT_USER_EMAIL
  repo account list

  repo codex NAME
  repo claude NAME
  repo open NAME [codex|claude]

environment:
  REPO_LLM_CONFIG      config root (default: $XDG_CONFIG_HOME/repo-llm or ~/.config/repo-llm)
  REPO_LLM_KEYS        deploy key directory (default: ~/.ssh/deploy_keys)
  REPO_LLM_CODE        clone directory (default: ~/code)
  REPO_LLM_CODEX_CMD   tmux command for `repo codex`
  REPO_LLM_CLAUDE_CMD  tmux command for `repo claude`

examples:
  repo account add main \"Your Name\" \"you@example.com\"
  repo register windlass ~/code/windlass main
  repo add git@github.com:owner/parts.git
  repo key parts
  repo clone parts
  repo codex parts"
    );
}

impl Paths {
    fn from_env() -> Result<Self> {
        let home = home_dir()?;
        let xdg_config = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let app_dir = env::var_os("REPO_LLM_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| xdg_config.join("repo-llm"));
        let key_dir = env::var_os("REPO_LLM_KEYS")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".ssh").join("deploy_keys"));
        let code_dir = env::var_os("REPO_LLM_CODE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("code"));

        Ok(Self {
            repo_dir: app_dir.join("repos.d"),
            account_dir: app_dir.join("accounts.d"),
            app_dir,
            key_dir,
            code_dir,
            ssh_config: home.join(".ssh").join("config"),
            ssh_config_dir: home.join(".ssh").join("config.d"),
        })
    }

    fn repo_config_path(&self, name: &str) -> PathBuf {
        self.repo_dir.join(format!("{name}.toml"))
    }

    fn account_config_path(&self, name: &str) -> PathBuf {
        self.account_dir.join(format!("{name}.toml"))
    }
}

fn cmd_account_add(
    paths: &Paths,
    name: &str,
    git_user_name: &str,
    git_user_email: &str,
) -> Result<()> {
    ensure_dirs(paths)?;
    validate_name(name, "account")?;

    write_toml_file(
        &paths.account_config_path(name),
        &AccountConfig {
            git_user_name: git_user_name.to_string(),
            git_user_email: git_user_email.to_string(),
        },
    )?;
    println!("Saved account: {name}");
    Ok(())
}

fn cmd_account_list(paths: &Paths) -> Result<()> {
    ensure_dirs(paths)?;
    for name in list_config_names(&paths.account_dir)? {
        println!("{name}");
    }
    Ok(())
}

fn cmd_register(paths: &Paths, name: &str, project: &str, account: Option<&str>) -> Result<()> {
    ensure_dirs(paths)?;
    validate_name(name, "repo")?;

    let project = normalize_path(project)?;
    if !project.join(".git").is_dir() {
        return Err(format!("not a git repo: {}", project.display()));
    }

    let git_url = command_output(
        Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["remote", "get-url", "origin"]),
    )
    .map_err(|_| format!("repo has no origin remote: {}", project.display()))?;

    let repo = RepoConfig {
        name: RepoName(name.to_string()),
        project: project.clone(),
        git_url: GitHubRepoUrl(git_url.clone()),
        clone_url: git_url,
        account: account
            .map(str::to_string)
            .filter(|value| !value.is_empty())
            .map(AccountName),
        key_file: None,
        host_alias: None,
    };
    write_repo_config(paths, &repo)?;
    apply_account(paths, &project, account)?;

    println!("Registered repo: {name}");
    println!("Project: {}", project.display());
    Ok(())
}

fn cmd_add(paths: &Paths, git_url: &str) -> Result<()> {
    need("ssh-keygen")?;
    ensure_ssh_include(paths)?;

    let github = github_repo_from_url(git_url)?;
    let name = github.repo.as_str();
    let account = github.owner.as_str();

    validate_name(name, "repo")?;
    validate_name(account, "account")?;

    let project = paths.code_dir.join(name);
    let host_alias = format!("github-{name}");
    let key_file = paths.key_dir.join(name);
    let clone_url = format!("git@{host_alias}:{}", github.path);
    let ssh_file = paths.ssh_config_dir.join(format!("repo-llm-{name}.conf"));
    ensure_add_targets_available(paths, name, &key_file, &ssh_file)?;

    if !key_file.exists() {
        let status = Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-C"])
            .arg(format!("deploy:{name}"))
            .arg("-f")
            .arg(&key_file)
            .args(["-N", ""])
            .status()
            .map_err(|err| format!("failed to run ssh-keygen: {err}"))?;
        if !status.success() {
            return Err(format!("ssh-keygen failed with status {status}"));
        }
    }

    chmod_if_exists(&key_file, 0o600);
    chmod_if_exists(key_file.with_extension("pub"), 0o644);

    write_file(
        &ssh_file,
        &format!(
            "Host {host_alias}\n  HostName github.com\n  User git\n  IdentityFile {}\n  IdentitiesOnly yes\n",
            key_file.display()
        ),
    )?;
    chmod_if_exists(&ssh_file, 0o600);

    let repo = RepoConfig {
        name: RepoName(name.to_string()),
        project,
        git_url: GitHubRepoUrl(git_url.to_string()),
        clone_url,
        account: Some(AccountName(account.to_string())),
        key_file: Some(key_file.clone()),
        host_alias: Some(SshHostAlias(host_alias)),
    };
    write_repo_config(paths, &repo)?;

    println!("Created repo config: {name}");
    println!();
    println!("Paste this public key into GitHub as a deploy key for:");
    println!("  {}", github.path);
    println!();
    print_file(key_file.with_extension("pub"))?;
    println!();
    println!("After pasting the key:");
    println!("  repo clone {name}");
    println!("  repo codex {name}");
    Ok(())
}

fn ensure_add_targets_available(
    paths: &Paths,
    name: &str,
    key_file: &Path,
    ssh_file: &Path,
) -> Result<()> {
    let public_key = key_file.with_extension("pub");
    let candidates = [
        ("repo config", paths.repo_config_path(name)),
        ("private key", key_file.to_path_buf()),
        ("public key", public_key),
        ("ssh config", ssh_file.to_path_buf()),
    ];
    let existing: Vec<String> = candidates
        .into_iter()
        .filter(|(_, path)| path.exists())
        .map(|(label, path)| format!("{label}: {}", path.display()))
        .collect();

    if existing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "refusing to overwrite existing repo add files:\n  {}",
            existing.join("\n  ")
        ))
    }
}

fn cmd_key(paths: &Paths, name: &str) -> Result<()> {
    let repo = load_repo(paths, name)?;
    let key_file = repo
        .key_file
        .ok_or_else(|| "repo was registered without a managed deploy key".to_string())?;
    let public_key = key_file.with_extension("pub");
    if !public_key.is_file() {
        return Err(format!("missing public key: {}", public_key.display()));
    }
    print_file(public_key)
}

fn cmd_clone(paths: &Paths, name: &str) -> Result<()> {
    need("git")?;
    let repo = load_repo(paths, name)?;

    if repo.project.join(".git").is_dir() {
        println!("Already cloned: {}", repo.project.display());
        return Ok(());
    }

    if let Some(parent) = repo.project.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }

    let status = Command::new("git")
        .arg("clone")
        .arg(&repo.clone_url)
        .arg(&repo.project)
        .status()
        .map_err(|err| format!("failed to run git clone: {err}"))?;
    if !status.success() {
        return Err(format!("git clone failed with status {status}"));
    }

    apply_account(
        paths,
        &repo.project,
        repo.account.as_ref().map(AccountName::as_str),
    )?;
    println!("Cloned: {}", repo.project.display());
    Ok(())
}

fn cmd_list(paths: &Paths) -> Result<()> {
    ensure_dirs(paths)?;
    for name in list_config_names(&paths.repo_dir)? {
        let repo = load_repo(paths, &name)?;
        println!("{:<20} {}", repo.name.as_str(), repo.project.display());
    }
    Ok(())
}

fn cmd_open(paths: &Paths, name: &str, llm: Llm) -> Result<()> {
    need("tmux")?;
    let repo = load_repo(paths, name)?;

    if !repo.project.is_dir() {
        return Err(format!(
            "project directory does not exist: {}",
            repo.project.display()
        ));
    }
    if !repo.project.join(".git").is_dir() {
        return Err(format!(
            "project is not cloned as git repo: {}",
            repo.project.display()
        ));
    }

    let session = format!("{name}-{}", llm_name(llm));
    if Command::new("tmux")
        .args(["has-session", "-t", &session])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
    {
        return run_foreground(Command::new("tmux").args(["attach-session", "-t", &session]));
    }

    let command = cmd_for_llm(llm);
    run_foreground(
        Command::new("tmux")
            .args(["new-session", "-s", &session, "-c"])
            .arg(&repo.project)
            .arg(command),
    )
}

fn load_repo(paths: &Paths, name: &str) -> Result<RepoConfig> {
    validate_name(name, "repo")?;
    let file = paths.repo_config_path(name);
    if !file.is_file() {
        return Err(format!("unknown repo: {name}"));
    }

    let repo: RepoConfig = read_toml_file(&file)?;
    if repo.name.as_str().is_empty() {
        return Err("broken repo config: missing name".to_string());
    }
    if repo.clone_url.is_empty() {
        return Err("broken repo config: missing clone_url".to_string());
    }
    Ok(repo)
}

fn load_account_if_present(paths: &Paths, account: Option<&str>) -> Result<Option<AccountConfig>> {
    let Some(account) = account.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let file = paths.account_config_path(account);
    if !file.is_file() {
        return Ok(None);
    }

    Ok(Some(read_toml_file(&file)?))
}

fn write_repo_config(paths: &Paths, repo: &RepoConfig) -> Result<()> {
    write_toml_file(&paths.repo_config_path(repo.name.as_str()), repo)
}

fn apply_account(paths: &Paths, project: &Path, account: Option<&str>) -> Result<()> {
    let Some(account) = load_account_if_present(paths, account)? else {
        return Ok(());
    };

    command_ok(
        Command::new("git").arg("-C").arg(project).args([
            "config",
            "user.name",
            &account.git_user_name,
        ]),
        "git config user.name",
    )?;
    command_ok(
        Command::new("git").arg("-C").arg(project).args([
            "config",
            "user.email",
            &account.git_user_email,
        ]),
        "git config user.email",
    )
}

fn ensure_dirs(paths: &Paths) -> Result<()> {
    let home = home_dir()?;
    for path in [
        paths.app_dir.as_path(),
        paths.repo_dir.as_path(),
        paths.account_dir.as_path(),
        paths.key_dir.as_path(),
        paths.ssh_config_dir.as_path(),
        paths.code_dir.as_path(),
    ] {
        fs::create_dir_all(path)
            .map_err(|err| format!("failed to create {}: {err}", path.display()))?;
    }
    chmod_if_exists(home.join(".ssh"), 0o700);
    chmod_if_exists(&paths.key_dir, 0o700);
    chmod_if_exists(&paths.ssh_config_dir, 0o700);
    Ok(())
}

fn ensure_ssh_include(paths: &Paths) -> Result<()> {
    ensure_dirs(paths)?;

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.ssh_config)
        .map_err(|err| format!("failed to touch {}: {err}", paths.ssh_config.display()))?;
    chmod_if_exists(&paths.ssh_config, 0o600);

    let mut current = String::new();
    File::open(&paths.ssh_config)
        .and_then(|mut file| file.read_to_string(&mut current))
        .map_err(|err| format!("failed to read {}: {err}", paths.ssh_config.display()))?;

    let has_include = current.lines().any(|line| {
        let mut parts = line.split_whitespace();
        matches!(parts.next(), Some("Include"))
            && parts.any(|part| part == "~/.ssh/config.d/*.conf")
    });

    if !has_include {
        write_file(
            &paths.ssh_config,
            &format!("Include ~/.ssh/config.d/*.conf\n{current}"),
        )?;
        chmod_if_exists(&paths.ssh_config, 0o600);
    }
    Ok(())
}

fn github_repo_from_url(url: &str) -> Result<GitHubRepo> {
    let path = if let Some(path) = url.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = url.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = url.strip_prefix("http://github.com/") {
        path
    } else {
        return Err("only github.com URLs are supported here".to_string());
    };

    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("could not parse GitHub owner/repo from: {url}"));
    };
    if owner.is_empty()
        || repo.is_empty()
        || owner.chars().any(char::is_whitespace)
        || repo.chars().any(char::is_whitespace)
    {
        return Err(format!("could not parse GitHub owner/repo from: {url}"));
    }
    Ok(GitHubRepo {
        owner: owner.to_string(),
        repo: repo.to_string(),
        path: format!("{owner}/{repo}.git"),
    })
}

fn validate_name(name: &str, kind: &str) -> Result<()> {
    if !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        Ok(())
    } else {
        Err(format!("bad {kind} name: {name}"))
    }
}

fn cmd_for_llm(llm: Llm) -> String {
    match llm {
        Llm::Codex => {
            env::var("REPO_LLM_CODEX_CMD").unwrap_or_else(|_| DEFAULT_CODEX_CMD.to_string())
        }
        Llm::Claude => {
            env::var("REPO_LLM_CLAUDE_CMD").unwrap_or_else(|_| DEFAULT_CLAUDE_CMD.to_string())
        }
    }
}

fn llm_name(llm: Llm) -> &'static str {
    match llm {
        Llm::Codex => "codex",
        Llm::Claude => "claude",
    }
}

fn read_toml_file<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut content = String::new();
    File::open(path)
        .and_then(|mut file| file.read_to_string(&mut content))
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    toml::from_str(&content).map_err(|err| format!("failed to parse {}: {err}", path.display()))
}

fn write_toml_file<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    let content =
        toml::to_string_pretty(value).map_err(|err| format!("failed to encode TOML: {err}"))?;
    write_file(path, &content)
}

fn list_config_names(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in
        fs::read_dir(dir).map_err(|err| format!("failed to read {}: {err}", dir.display()))?
    {
        let entry =
            entry.map_err(|err| format!("failed to read entry in {}: {err}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("toml") {
            if let Some(name) = path.file_stem().and_then(|value| value.to_str()) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

fn normalize_path(value: &str) -> Result<PathBuf> {
    let expanded = if value == "~" {
        home_dir()?
    } else if let Some(rest) = value.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        PathBuf::from(value)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        env::current_dir()
            .map_err(|err| format!("failed to read current directory: {err}"))?
            .join(expanded)
    };
    Ok(normalize_components(&absolute))
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "HOME is not set".to_string())
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let mut file =
        File::create(path).map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    file.write_all(content.as_bytes())
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

fn print_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let mut content = String::new();
    File::open(path)
        .and_then(|mut file| file.read_to_string(&mut content))
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    print!("{content}");
    Ok(())
}

fn chmod_if_exists(path: impl AsRef<Path>, mode: u32) {
    let path = path.as_ref();
    if path.exists() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

fn need(program: &str) -> Result<()> {
    if find_in_path(program) {
        Ok(())
    } else {
        Err(format!("missing command: {program}"))
    }
}

fn find_in_path(program: &str) -> bool {
    if program.contains('/') {
        return is_executable(Path::new(program));
    }
    env::var_os("PATH")
        .is_some_and(|path| env::split_paths(&path).any(|dir| is_executable(&dir.join(program))))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn command_ok(command: &mut Command, name: &str) -> Result<()> {
    let status = command
        .status()
        .map_err(|err| format!("failed to run {name}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed with status {status}"))
    }
}

fn command_output(command: &mut Command) -> std::result::Result<String, ()> {
    let output = command.output().map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn run_foreground(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .map_err(|err| format!("failed to run command: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed with status {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_urls() {
        assert_eq!(
            github_repo_from_url("git@github.com:owner/project.git").unwrap(),
            GitHubRepo {
                owner: "owner".to_string(),
                repo: "project".to_string(),
                path: "owner/project.git".to_string()
            }
        );
        assert_eq!(
            github_repo_from_url("https://github.com/owner/project").unwrap(),
            GitHubRepo {
                owner: "owner".to_string(),
                repo: "project".to_string(),
                path: "owner/project.git".to_string()
            }
        );
        assert!(github_repo_from_url("https://example.com/owner/project").is_err());
    }

    #[test]
    fn validates_config_names() {
        assert!(validate_name("repo_1.2-test", "repo").is_ok());
        assert!(validate_name("../nope", "repo").is_err());
        assert!(validate_name("", "repo").is_err());
    }

    #[test]
    fn writes_repo_config_as_toml() {
        let root = temp_test_dir("repo-cli-toml");
        let paths = Paths {
            app_dir: root.join("config"),
            repo_dir: root.join("config/repos.d"),
            account_dir: root.join("config/accounts.d"),
            key_dir: root.join("keys"),
            code_dir: root.join("code"),
            ssh_config: root.join(".ssh/config"),
            ssh_config_dir: root.join(".ssh/config.d"),
        };
        fs::create_dir_all(&paths.repo_dir).unwrap();

        let repo = RepoConfig {
            name: RepoName("windlass".to_string()),
            project: PathBuf::from("/home/ofweb/code/windlass"),
            git_url: GitHubRepoUrl("git@github.com:ofweb/windlass.git".to_string()),
            clone_url: "git@github-windlass:ofweb/windlass.git".to_string(),
            account: Some(AccountName("ofweb".to_string())),
            key_file: Some(PathBuf::from("/home/ofweb/.ssh/deploy_keys/windlass")),
            host_alias: Some(SshHostAlias("github-windlass".to_string())),
        };

        write_repo_config(&paths, &repo).unwrap();
        let content = fs::read_to_string(paths.repo_config_path("windlass")).unwrap();
        assert!(content.contains("name = \"windlass\""));
        assert!(content.contains("project = \"/home/ofweb/code/windlass\""));
        assert!(content.contains("account = \"ofweb\""));

        let loaded = load_repo(&paths, "windlass").unwrap();
        assert_eq!(loaded.name.as_str(), "windlass");
        assert_eq!(loaded.git_url.0, "git@github.com:ofweb/windlass.git");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_cli_open_defaulting_to_codex() {
        assert_eq!(
            parse_cli(vec!["open".into(), "windlass".into()]).unwrap(),
            Cli::Open {
                name: "windlass".to_string(),
                llm: Llm::Codex
            }
        );
    }

    #[test]
    fn parses_cli_add_from_url_only() {
        assert_eq!(
            parse_cli(vec!["add".into(), "git@github.com:ofweb/repo.git".into()]).unwrap(),
            Cli::Add {
                github_url: "git@github.com:ofweb/repo.git".to_string()
            }
        );
    }

    #[test]
    fn add_refuses_to_overwrite_existing_files() {
        let root = temp_test_dir("repo-cli-test");
        let paths = Paths {
            app_dir: root.join("config"),
            repo_dir: root.join("config/repos.d"),
            account_dir: root.join("config/accounts.d"),
            key_dir: root.join("keys"),
            code_dir: root.join("code"),
            ssh_config: root.join(".ssh/config"),
            ssh_config_dir: root.join(".ssh/config.d"),
        };
        fs::create_dir_all(&paths.repo_dir).unwrap();

        let repo_config = paths.repo_config_path("project");
        write_file(&repo_config, "name = \"project\"\n").unwrap();

        let key_file = paths.key_dir.join("project");
        let ssh_file = paths.ssh_config_dir.join("repo-llm-project.conf");
        let err =
            ensure_add_targets_available(&paths, "project", &key_file, &ssh_file).unwrap_err();

        assert!(err.contains("refusing to overwrite"));
        assert!(err.contains(&repo_config.display().to_string()));

        fs::remove_dir_all(root).unwrap();
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
