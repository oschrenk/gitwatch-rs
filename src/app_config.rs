use crate::{cli::CliOptions, config_file::ConfigFile, util::normalize_path};
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Default)]
pub struct AppConfig {
    pub commit_message: Option<String>,
    pub commit_message_script: Option<PathBuf>,
    pub commit_on_start: bool,
    pub debounce_seconds: u64,
    pub dry_run: bool,
    pub ignore_regex: Option<Regex>,
    /// Resolved from either `ntfy_url` or the content of `ntfy_url_file`.
    pub ntfy_url: Option<String>,
    pub pull_before_push: bool,
    pub pull_interval_seconds: Option<u64>,
    pub remote: Option<String>,
    pub repository: PathBuf,
    pub retries: i32,
    pub skip_if_merging: bool,
    pub ssh_key: Option<PathBuf>,
    pub watch: bool,
}

impl AppConfig {
    pub fn new(cli_config: CliOptions) -> Result<Self> {
        // load config file if it exists
        let file_config = ConfigFile::load(&cli_config.repository).unwrap_or_default();

        let repository = normalize_path(&cli_config.repository).context(format!(
            "Invalid repository path '{}'",
            cli_config.repository.display()
        ))?;

        let config = Self::merge_configs(repository, cli_config, file_config)?;

        config.validate()?;
        Ok(config)
    }

    // merge with precedence: config file > cli flags
    fn merge_configs(
        repository: PathBuf,
        cli_config: CliOptions,
        file_config: ConfigFile,
    ) -> Result<Self> {
        let commit_message = file_config
            .commit_message
            .or(cli_config.commit_message.message);

        let commit_message_script = file_config
            .commit_message_script
            .or(cli_config.commit_message.script)
            .map(|script_path| {
                let script_path = if script_path.is_relative() {
                    // if relative path, interpret it relative to repository root
                    repository.join(script_path)
                } else {
                    script_path
                };
                normalize_path(&script_path).context(format!(
                    "Invalid commit message script path '{}'",
                    script_path.display()
                ))
            })
            .transpose()?;

        let commit_on_start = file_config
            .commit_on_start
            .unwrap_or(cli_config.commit_on_start);

        let debounce_seconds = file_config
            .debounce_seconds
            .unwrap_or(cli_config.debounce_seconds);

        let dry_run = file_config.dry_run.unwrap_or(cli_config.dry_run);

        let ignore_regex = if let Some(regex) = file_config.ignore_regex {
            Some(regex)
        } else {
            cli_config.ignore_regex
        };

        let ntfy_url = match (
            file_config.ntfy_url.or(cli_config.ntfy_url),
            file_config.ntfy_url_file.or(cli_config.ntfy_url_file),
        ) {
            (Some(_), Some(_)) => bail!("Only one of ntfy-url or ntfy-url-file can be set"),
            (url, None) => url,
            (None, Some(file_path)) => {
                let file_path = normalize_path(&file_path).context(format!(
                    "Invalid ntfy URL file path '{}'",
                    file_path.display()
                ))?;
                let url = fs::read_to_string(&file_path).context(format!(
                    "Failed to read ntfy URL file '{}'",
                    file_path.display()
                ))?;
                Some(url.trim().to_string())
            }
        };

        let pull_before_push = file_config
            .pull_before_push
            .unwrap_or(cli_config.pull_before_push);

        let pull_interval_seconds = file_config
            .pull_interval_seconds
            .or(cli_config.pull_interval_seconds);

        let remote = if let Some(remote) = file_config.remote {
            Some(remote)
        } else {
            cli_config.remote
        };

        let retries = file_config.retries.unwrap_or(cli_config.retries);

        let skip_if_merging = file_config
            .skip_if_merging
            .unwrap_or(cli_config.skip_if_merging);

        let ssh_key = file_config
            .ssh_key
            .or(cli_config.ssh_key)
            .map(|key_path| {
                normalize_path(&key_path)
                    .context(format!("Invalid SSH key path '{}'", key_path.display()))
            })
            .transpose()?;

        let watch = file_config.watch.unwrap_or(cli_config.watch);

        Ok(Self {
            repository,
            commit_message,
            commit_message_script,
            commit_on_start,
            debounce_seconds,
            dry_run,
            ignore_regex,
            ntfy_url,
            pull_before_push,
            pull_interval_seconds,
            remote,
            retries,
            skip_if_merging,
            ssh_key,
            watch,
        })
    }

    fn validate(&self) -> Result<()> {
        if self.retries < -1 {
            bail!("Retry count must be >= -1");
        }

        if self.pull_before_push && self.remote.is_none() {
            bail!("pull-before-push requires a remote to be set");
        }

        match self.pull_interval_seconds {
            Some(_) if self.remote.is_none() => {
                bail!("pull-interval-seconds requires a remote to be set")
            }
            Some(0) => bail!("pull-interval-seconds must be > 0"),
            _ => {}
        }

        if let Some(url) = &self.ntfy_url {
            if self.remote.is_none() {
                bail!("ntfy-url requires a remote to be set");
            }
            if !url.starts_with("https://") && !url.starts_with("http://") {
                bail!("ntfy-url must start with https:// or http://");
            }
        }

        if !self.repository.exists() {
            bail!(
                "Repository path does not exist: {}",
                self.repository.display()
            );
        }

        if let Some(key_path) = &self.ssh_key {
            if !key_path.is_file() {
                bail!("SSH key path is not a file: {}", key_path.display());
            }
        }

        match (&self.commit_message, &self.commit_message_script) {
            (None, None) => {
                bail!("Either commit-message or commit-message-script must be set")
            }
            (Some(_), Some(_)) => {
                bail!("Only one of commit-message or commit-message-script can be set")
            }
            (None, Some(script_path)) => {
                if !script_path.exists() {
                    bail!(
                        "Commit message script does not exist: {}",
                        script_path.display()
                    );
                }
                if !script_path.is_file() {
                    bail!(
                        "Commit message script path is not a file: {}",
                        script_path.display()
                    );
                }
                Ok(())
            }
            (Some(_), None) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::Path, str::FromStr};

    use clap::Parser;
    use testresult::TestResult;

    use super::*;
    use crate::{
        cli::{CommitMessageOptions, LogLevel},
        test_support::constants::TEST_COMMIT_MESSAGE,
    };

    impl PartialEq for AppConfig {
        fn eq(&self, other: &Self) -> bool {
            self.repository
                .as_path()
                .canonicalize()
                .unwrap_or(self.repository.clone())
                == other
                    .repository
                    .as_path()
                    .canonicalize()
                    .unwrap_or(other.repository.clone())
                && self.ignore_regex.as_ref().map(|r| r.as_str())
                    == other.ignore_regex.as_ref().map(|r| r.as_str())
                && self.commit_message == other.commit_message
                && self.commit_message_script == other.commit_message_script
                && self.debounce_seconds == other.debounce_seconds
                && self.dry_run == other.dry_run
                && self.retries == other.retries
                && self.commit_on_start == other.commit_on_start
                && self.pull_before_push == other.pull_before_push
                && self.skip_if_merging == other.skip_if_merging
                && self.watch == other.watch
        }
    }

    #[test]
    fn test_config_from_cli() -> TestResult {
        let temp_dir = tempfile::tempdir()?;

        let watch_opts = CliOptions::parse_from([
            "gitwatch",
            temp_dir.path().to_str().unwrap(),
            "--commit-message",
            TEST_COMMIT_MESSAGE,
            "--debounce-seconds=0",
            "--ignore-regex=/ignore-me/.*",
            "--retries=2",
            "--commit-on-start=false",
            "--watch=true",
            "--dry-run",
            "--remote=origin",
            "--pull-before-push",
            "--skip-if-merging",
        ]);

        let config = AppConfig::new(watch_opts)?;

        let expected = AppConfig {
            repository: temp_dir.path().to_path_buf(),
            commit_message: Some(TEST_COMMIT_MESSAGE.to_string()),
            commit_message_script: None,
            debounce_seconds: 0,
            ignore_regex: Some(Regex::new("/ignore-me/.*")?),
            dry_run: true,
            retries: 2,
            ssh_key: None,
            commit_on_start: false,
            watch: true,
            ntfy_url: None,
            pull_before_push: true,
            pull_interval_seconds: None,
            remote: Some("origin".to_string()),
            skip_if_merging: true,
        };

        assert_eq!(config, expected);
        Ok(())
    }

    #[test]
    fn test_config_from_cli_invalid() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let invalid_watch_opts = CliOptions::parse_from([
            "gitwatch",
            temp_dir.path().to_str().unwrap(),
            "--commit-message",
            TEST_COMMIT_MESSAGE,
            "--retries=-2",
        ]);

        let result = AppConfig::new(invalid_watch_opts);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Retry count must be >= -1"));

        Ok(())
    }

    #[test]
    fn test_config_missing_ssh_key() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let missing_key = temp_dir.path().join("no-such-key");
        let opts = CliOptions::parse_from([
            "gitwatch",
            temp_dir.path().to_str().unwrap(),
            "--commit-message",
            TEST_COMMIT_MESSAGE,
            "--ssh-key",
            missing_key.to_str().unwrap(),
        ]);

        let err = AppConfig::new(opts).unwrap_err().to_string();
        assert!(err.contains("Invalid SSH key path"), "{err}");

        let opts = CliOptions::parse_from([
            "gitwatch",
            temp_dir.path().to_str().unwrap(),
            "--commit-message",
            TEST_COMMIT_MESSAGE,
            "--ssh-key",
            temp_dir.path().to_str().unwrap(),
        ]);
        let err = AppConfig::new(opts).unwrap_err().to_string();
        assert!(err.contains("SSH key path is not a file"), "{err}");

        Ok(())
    }

    #[test]
    fn test_config_ntfy_url_file() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let repo = temp_dir.path().to_str().unwrap();
        let url_file = temp_dir.path().join("ntfy-url");
        fs::write(&url_file, "https://ntfy.sh/topic\n")?;
        let url_file = url_file.to_str().unwrap();

        let opts = CliOptions::parse_from([
            "gitwatch",
            repo,
            "--commit-message",
            TEST_COMMIT_MESSAGE,
            "--remote=origin",
            "--ntfy-url-file",
            url_file,
        ]);
        let config = AppConfig::new(opts)?;
        assert_eq!(config.ntfy_url.as_deref(), Some("https://ntfy.sh/topic"));

        let both_flags = CliOptions::try_parse_from([
            "gitwatch",
            repo,
            "--ntfy-url=https://ntfy.sh/topic",
            "--ntfy-url-file",
            url_file,
        ]);
        assert!(both_flags.is_err());

        let missing_file = CliOptions::parse_from([
            "gitwatch",
            repo,
            "--ntfy-url-file",
            temp_dir.path().join("missing").to_str().unwrap(),
        ]);
        let err = AppConfig::new(missing_file).unwrap_err().to_string();
        assert!(err.contains("Invalid ntfy URL file path"), "{err}");

        fs::write(
            temp_dir.path().join("gitwatch.yaml"),
            "ntfy_url: \"https://ntfy.sh/topic\"",
        )?;
        let file_url_and_cli_file = CliOptions::parse_from([
            "gitwatch",
            repo,
            "--commit-message",
            TEST_COMMIT_MESSAGE,
            "--remote=origin",
            "--ntfy-url-file",
            url_file,
        ]);
        let err = AppConfig::new(file_url_and_cli_file)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Only one of ntfy-url or ntfy-url-file"),
            "{err}"
        );

        Ok(())
    }

    #[test]
    fn test_config_validation() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let repo_path = temp_dir.path().to_path_buf();

        let valid_script_path = temp_dir.path().join("commit-msg.sh");
        fs::write(&valid_script_path, "#!/bin/sh\necho 'test commit'")?;

        let valid_config = AppConfig {
            repository: repo_path.clone(),
            commit_message: Some("test".to_string()),
            commit_message_script: None,
            commit_on_start: true,
            debounce_seconds: 0,
            ignore_regex: None,
            watch: true,
            retries: 3,
            ssh_key: None,
            dry_run: false,
            ntfy_url: None,
            pull_before_push: false,
            pull_interval_seconds: None,
            remote: None,
            skip_if_merging: false,
        };
        assert!(valid_config.validate().is_ok());

        let config_missing_commit_message_options = AppConfig {
            commit_message: None,
            commit_message_script: None,
            ..valid_config.clone()
        };
        assert_eq!(
            config_missing_commit_message_options
                .validate()
                .unwrap_err()
                .to_string(),
            "Either commit-message or commit-message-script must be set"
        );

        let config_with_both_commit_message_options = AppConfig {
            commit_message: Some("test".into()),
            commit_message_script: Some(valid_script_path.clone()),
            ..valid_config.clone()
        };
        assert_eq!(
            config_with_both_commit_message_options
                .validate()
                .unwrap_err()
                .to_string(),
            "Only one of commit-message or commit-message-script can be set"
        );

        let valid_config_with_script = AppConfig {
            commit_message: None,
            commit_message_script: Some(valid_script_path.clone()),
            ..valid_config.clone()
        };
        assert!(valid_config_with_script.validate().is_ok());

        let invalid_retry_count = AppConfig {
            retries: -2,
            ..valid_config.clone()
        };
        assert!(invalid_retry_count
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Retry count must be >= -1"));

        let nonexistent_script_path = AppConfig {
            commit_message: None,
            commit_message_script: Some(temp_dir.path().join("nonexistent.sh")),
            ..valid_config.clone()
        };
        assert!(nonexistent_script_path
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Commit message script does not exist"));

        let script_path_is_directory = AppConfig {
            commit_message: None,
            commit_message_script: Some(repo_path),
            ..valid_config.clone()
        };
        assert!(script_path_is_directory
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Commit message script path is not a file"));

        let pull_without_remote = AppConfig {
            pull_before_push: true,
            remote: None,
            ..valid_config.clone()
        };
        assert!(pull_without_remote
            .validate()
            .unwrap_err()
            .to_string()
            .contains("pull-before-push requires a remote to be set"));

        let pull_with_remote = AppConfig {
            pull_before_push: true,
            remote: Some("origin".to_string()),
            ..valid_config.clone()
        };
        assert!(pull_with_remote.validate().is_ok());

        let interval_without_remote = AppConfig {
            pull_interval_seconds: Some(60),
            remote: None,
            ..valid_config.clone()
        };
        assert!(interval_without_remote
            .validate()
            .unwrap_err()
            .to_string()
            .contains("pull-interval-seconds requires a remote to be set"));

        let zero_interval = AppConfig {
            pull_interval_seconds: Some(0),
            remote: Some("origin".to_string()),
            ..valid_config.clone()
        };
        assert!(zero_interval
            .validate()
            .unwrap_err()
            .to_string()
            .contains("pull-interval-seconds must be > 0"));

        let ntfy_without_remote = AppConfig {
            ntfy_url: Some("https://ntfy.sh/topic".to_string()),
            remote: None,
            ..valid_config.clone()
        };
        assert!(ntfy_without_remote
            .validate()
            .unwrap_err()
            .to_string()
            .contains("ntfy-url requires a remote to be set"));

        let ntfy_without_scheme = AppConfig {
            ntfy_url: Some("ntfy.sh/topic".to_string()),
            remote: Some("origin".to_string()),
            ..valid_config.clone()
        };
        assert!(ntfy_without_scheme
            .validate()
            .unwrap_err()
            .to_string()
            .contains("ntfy-url must start with https:// or http://"));

        let nonexistent_repo_path = AppConfig {
            repository: PathBuf::from("/nonexistent/path"),
            ..valid_config.clone()
        };
        assert!(nonexistent_repo_path
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Repository path does not exist"));

        Ok(())
    }

    #[test]
    fn test_relative_paths() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let repo_path = temp_dir.path();
        let _ = create_test_commit_message_script(repo_path)?;
        env::set_current_dir(repo_path)?;

        let cli_opts = CliOptions {
            repository: PathBuf::from_str(".")?,
            commit_message: CommitMessageOptions {
                message: None,
                script: Some(PathBuf::from_str("./commit-msg.sh")?),
            },
            commit_on_start: true,
            debounce_seconds: 0,
            ignore_regex: None,
            watch: true,
            retries: 3,
            ssh_key: None,
            dry_run: false,
            ntfy_url: None,
            ntfy_url_file: None,
            pull_before_push: false,
            pull_interval_seconds: None,
            remote: None,
            skip_if_merging: false,
            log_level: LogLevel::Info,
        };

        let config = AppConfig::new(cli_opts)?;

        assert!(config.repository.exists());
        assert!(config.commit_message_script.unwrap().exists());

        Ok(())
    }

    #[test]
    fn test_absolute_paths() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let repo_path = temp_dir.path();
        let commit_message_script_path = create_test_commit_message_script(repo_path)?;
        env::set_current_dir(repo_path)?;

        let cli_opts = CliOptions {
            repository: repo_path.to_path_buf(),
            commit_message: CommitMessageOptions {
                message: None,
                script: Some(commit_message_script_path.clone()),
            },
            commit_on_start: true,
            debounce_seconds: 0,
            ignore_regex: None,
            watch: true,
            retries: 3,
            ssh_key: None,
            dry_run: false,
            ntfy_url: None,
            ntfy_url_file: None,
            pull_before_push: false,
            pull_interval_seconds: None,
            remote: None,
            skip_if_merging: false,
            log_level: LogLevel::Info,
        };

        let config = AppConfig::new(cli_opts)?;

        assert!(config.repository.exists());
        assert!(config.commit_message_script.unwrap().exists());

        Ok(())
    }

    #[test]
    fn test_config_precedence_cli_only() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        let cli_opts = create_test_cli_options(temp_dir.path())?;
        let config = AppConfig::new(cli_opts)?;

        assert_eq!(config.commit_message.unwrap(), "cli message");
        assert_eq!(None, config.commit_message_script);
        assert!(config.commit_on_start);
        assert_eq!(config.debounce_seconds, 1);
        assert!(!config.dry_run);
        assert_eq!(config.ignore_regex.unwrap().as_str(), "cli_ignore.*");
        assert!(!config.pull_before_push);
        assert_eq!(config.remote.unwrap(), "cli_remote");
        assert_eq!(config.retries, 3);
        assert!(!config.skip_if_merging);
        assert!(config.watch);

        Ok(())
    }

    #[test]
    fn test_config_precedence_with_file() -> TestResult {
        let temp_dir = tempfile::tempdir()?;
        create_test_config_file(temp_dir.path())?;
        let cli_opts = create_test_cli_options(temp_dir.path())?;
        let config = AppConfig::new(cli_opts)?;

        assert_eq!(None, config.commit_message_script);

        // file should take precedence
        assert_eq!(config.commit_message.unwrap(), "file message");
        assert!(!config.commit_on_start);
        assert_eq!(config.debounce_seconds, 5);
        assert!(config.dry_run);
        assert_eq!(config.ignore_regex.unwrap().as_str(), "file_ignore.*");
        assert!(config.pull_before_push);
        assert_eq!(config.remote.unwrap(), "file_remote");
        assert_eq!(config.retries, 5);
        assert!(config.skip_if_merging);
        assert!(!config.watch);

        Ok(())
    }

    #[test]
    fn test_config_partial_file() -> TestResult {
        let temp_dir = tempfile::tempdir()?;

        // Create config file with only some fields
        let partial_config = r#"
        debounce_seconds: 5
        remote: "file_remote"
        "#;
        fs::write(temp_dir.path().join("gitwatch.yaml"), partial_config)?;

        let cli_opts = create_test_cli_options(temp_dir.path())?;
        let config = AppConfig::new(cli_opts)?;

        // file values should be used where present
        assert_eq!(config.debounce_seconds, 5);
        assert_eq!(config.remote.unwrap(), "file_remote");

        // CLI values should be used for missing fields
        assert!(config.commit_on_start);
        assert!(!config.dry_run);
        assert_eq!(config.ignore_regex.unwrap().as_str(), "cli_ignore.*");
        assert_eq!(config.retries, 3);

        Ok(())
    }

    fn create_test_cli_options(repo_path: &Path) -> Result<CliOptions> {
        Ok(CliOptions {
            repository: repo_path.to_path_buf(),
            commit_message: CommitMessageOptions {
                message: Some("cli message".to_string()),
                script: None,
            },
            commit_on_start: true,
            debounce_seconds: 1,
            dry_run: false,
            ignore_regex: Some(Regex::new("cli_ignore.*").unwrap()),
            log_level: LogLevel::Info,
            ntfy_url: None,
            ntfy_url_file: None,
            pull_before_push: false,
            pull_interval_seconds: None,
            remote: Some("cli_remote".to_string()),
            retries: 3,
            skip_if_merging: false,
            ssh_key: None,
            watch: true,
        })
    }

    fn create_test_config_file(dir: &Path) -> Result<()> {
        let config_content = r#"
        commit_message: "file message"
        commit_on_start: false
        debounce_seconds: 5
        dry_run: true
        ignore_regex: "file_ignore.*"
        log_level: "debug"
        pull_before_push: true
        remote: "file_remote"
        retries: 5
        skip_if_merging: true
        watch: false
        "#;

        fs::write(dir.join("gitwatch.yaml"), config_content)?;
        Ok(())
    }

    fn create_test_commit_message_script(repo_path: &Path) -> Result<PathBuf> {
        let script_path = repo_path.join("commit-msg.sh");
        fs::write(&script_path, "#!/bin/sh\necho 'test commit'")?;
        Ok(script_path)
    }
}
