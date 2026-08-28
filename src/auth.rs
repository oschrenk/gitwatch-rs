use std::path::PathBuf;

use auth_git2::GitAuthenticator;

/// How the daemon proves its identity to the remote.
///
/// Cloned into the thread that runs the network operation, so it owns its data
/// rather than borrowing from the repository.
#[derive(Clone, Debug, Default)]
pub struct AuthConfig {
    ssh_key: Option<PathBuf>,
}

impl AuthConfig {
    pub fn new(ssh_key: Option<PathBuf>) -> Self {
        Self { ssh_key }
    }

    /// Builds the authenticator for a network operation.
    ///
    /// With an explicit key, everything else is switched off. That matters more
    /// than it looks: `GitAuthenticator::default()` tries the SSH agent *first*
    /// and only falls back to key files if the agent attempt returns. A locked
    /// 1Password agent never returns, so the key would never be reached. Only
    /// `new_empty()` guarantees the agent is not contacted at all.
    ///
    /// It also leaves `try_password_prompt` at zero, so the daemon never tries
    /// to prompt for a passphrase on a terminal it does not have.
    ///
    /// Without a key, the default authenticator is kept, so existing setups -
    /// agent, credential helper, default key paths - keep working as before.
    pub fn authenticator(&self) -> GitAuthenticator {
        match &self.ssh_key {
            Some(key_path) => GitAuthenticator::new_empty()
                .add_default_username()
                .add_ssh_key_from_file(key_path.clone(), None),
            None => GitAuthenticator::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_key_disables_agent_and_prompts() {
        let auth = AuthConfig::new(Some(PathBuf::from("/keys/deploy_ed25519")));
        let debug = format!("{:?}", auth.authenticator());

        assert!(
            debug.contains("try_ssh_agent: false"),
            "The agent must be disabled, it is the thing that hangs: {debug}"
        );
        assert!(
            debug.contains("try_password_prompt: 0"),
            "A daemon has no terminal to prompt on: {debug}"
        );
        assert!(
            debug.contains("deploy_ed25519"),
            "The configured key must be used: {debug}"
        );
    }

    #[test]
    fn test_without_key_keeps_default_behaviour() {
        let debug = format!("{:?}", AuthConfig::default().authenticator());
        assert!(
            debug.contains("try_ssh_agent: true"),
            "Existing setups must keep using the agent: {debug}"
        );
    }
}
