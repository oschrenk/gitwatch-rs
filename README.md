<div align="center">

<h1>gitwatch-rs</h1>
<em>Watch a local Git repository and automatically commit changes</em><br/><br/>

[![codecov](https://codecov.io/gh/croissong/gitwatch-rs/graph/badge.svg?token=UBV2B6146B)](https://codecov.io/gh/croissong/gitwatch-rs)
[![GitHub Release](https://img.shields.io/github/v/release/croissong/gitwatch-rs)](https://github.com/croissong/gitwatch-rs/releases/latest)
[![crates](https://img.shields.io/crates/v/gitwatch-rs.svg)](https://crates.io/crates/gitwatch-rs)
[![Packaging status](https://repology.org/badge/tiny-repos/gitwatch-rs.svg)](https://repology.org/project/gitwatch-rs/versions)

</div>

A Rust implementation of the original [gitwatch](https://github.com/gitwatch/gitwatch) script - with a few additional features.

- [Features](#features)
- [Usage](#usage)
  - [Configuration](#configuration)
  - [Tips](#tips)
- [Installation](#installation)
- [Contributing](#contributing)
- [Credits](#credits)
- [Contributing](#contributing)

## Features

- Watch a local Git repository and automatically commit changes
- Optionally push to a remote 
- Use a custom commit message or generate one via a script
- Configure a debounce time to limit commit frequency 


## Usage

Basic usage:
```sh
gitwatch watch /path/to/repo --commit-message "Auto commit"
```

<details>
<summary><b>Example use case</b></summary>

I use gitwatch to watch my local notes repository and generate commit messages using [aichat](https://github.com/sigoden/aichat).  
The [example](example/) folder contains a small repository demonstrating this use case:

<img src="docs/example.png" alt="Example use case">

</details>

### Configuration

```console
❯ gitwatch watch --help
Watch a repository and commit changes

Usage: gitwatch watch [OPTIONS] [REPOSITORY]

Arguments:
  [REPOSITORY]  Path to the Git repository to monitor for changes [default: .]

Options:
  -m, --commit-message <MESSAGE>
          Static commit message to use for all commits
      --commit-message-script <SCRIPT>
          Path to executable script that generates commit messages.
          The path can be absolute or relative to the repository.
          The script is executed with the repository as working directory
          and must output the message to stdout.
      --commit-on-start <COMMIT_ON_START>
          Automatically commit any existing changes on start [default: true] [possible values: true, false]
      --debounce-seconds <DEBOUNCE_SECONDS>
          Number of seconds to wait before processing multiple changes to the same file.
          Higher values reduce commit frequency but group more changes together. [default: 1]
      --dry-run
          Run without performing actual Git operations (staging, committing, etc.)
  -i, --ignore-regex <IGNORE_REGEX>
          Regular expression pattern for files to exclude from watching.
          Matching is performed against repository-relative file paths.
          Note: the .git folder & gitignored files are ignored by default.
          Example: "\.tmp$" to ignore temporary files.
      --log-level <LOG_LEVEL>
          Set the log level [default: info] [possible values: trace, debug, info, warn, error]
      --ntfy-url <NTFY_URL>
          URL of an ntfy topic to stream (requires --remote).
          Each message on the topic triggers a pull, so a push to the remote
          arrives without polling.
          Example: "https://ntfy.sh/my-random-topic".
      --ntfy-url-file <NTFY_URL_FILE>
          Path to a file that holds the ntfy topic URL.
          Keeps the URL out of the command line and service definitions.
      --pull-before-push
          Pull from the remote before pushing (requires --remote).
          Fetches the current branch and rebases the local commits on top of it,
          so a remote that has moved ahead does not break the push.
      --pull-interval-seconds <PULL_INTERVAL_SECONDS>
          Pull from the remote every N seconds while no change is pending
          (requires --remote). Off when unset.
  -r, --remote <REMOTE>
          Name of the remote to push to (if specified).
          Example: "origin".
      --retries <RETRIES>
          Number of retry attempts when errors occur.
          Use -1 for infinite retries. [default: 3]
      --skip-if-merging
          Skip committing while a merge, rebase, cherry-pick,
          revert or bisect is in progress.
          Prevents gitwatch from committing unresolved conflicts.
      --ssh-key <SSH_KEY>
          Path to the SSH private key to authenticate with.
          Disables the SSH agent, so a locked agent cannot block the push.
  -w, --watch <WATCH>
          Enable continuous monitoring of filesystem changes.
          Set to false for one-time commit of current changes. [default: true] [possible values: true, false]
  -h, --help
          Print help
```

#### Config file

Most options can also be configured in a `gitwatch.yml` file located at the root of the watched repository.
See [docs/gitwatch.example.yaml](docs/gitwatch.example.yaml) for reference.


### Tips

<details><summary><b>Disable GPG commit signing for your watched repo</b></summary>

If you've enabled `gpgsign` globally, you might want to disable it for the watched repositories, since `gitwatch` uses your regular git user to create the commits.

1. Add an include to a custom `.gitconfig` file to your local gitconfig:
   ```sh
   # cd /path/to/repo 
   git config --local include.path ../.gitconfig
   ```
2. Create a .gitconfig file (which can be committed):
   ```gitconfig
   [commit]
     gpgsign = false
   ```

</details>

<details><summary><b>Run as systemd service</b></summary>

Create a systemd user service file `gitwatch@.service`:
```ini
[Unit]
Description=Watch a Git repository and automatically commit changes

[Service]
ExecStart=/usr/local/bin/gitwatch watch %I
ExecStop=/bin/true

[Install]
WantedBy=default.target
```

I recommend you use it in combination with a config file, then you only need to pass the repo dir as argument:
```sh
systemctl --user start gitwatch@$(systemd-escape /path/to/repo/).service
```
   
   
There's also a [Nix module](nix/service.nix), which provides a home-manager service:
```nix
inputs = {
   gitwatch-rs.url = "github:croissong/gitwatch-rs";
};

...
  
imports = [ inputs.gitwatch-rs.modules.gitwatch ];

services.gitwatch = {
  notes = {
    repo_path = "${config.home.homeDirectory}/notes/";
    args = [ "--log-level=debug" ];
    extraPackages = with pkgs; [
      bash
      coreutils
      git
      aichat
    ];
  };
}
```

</details>

<details><summary><b>Pull on push with ntfy</b></summary>

With `--ntfy-url`, gitwatch keeps one outbound connection to an [ntfy](https://ntfy.sh) topic and pulls when a message arrives.
A GitHub webhook posts to the topic on every push, so the watched machine needs no open port and no polling.
`--pull-interval-seconds` still works as a slow fallback.

1. Pick a long random topic. Anyone who knows the topic URL can read it and trigger a pull.
   ```sh
   echo "https://ntfy.sh/gitwatch-$(openssl rand -hex 16)"
   ```
2. Add a webhook to the GitHub repository.
   The `tpl` and `m` query parameters make ntfy keep only `push <ref>` from the GitHub payload.
   Without them, ntfy stores the whole payload, commit messages and author emails included, as a public attachment.
   ```sh
   gh api repos/<owner>/<repo>/hooks \
     -f name=web \
     -f "config[url]=<topic-url>?tpl=1&m=push%20%7B%7B.ref%7D%7D" \
     -f 'config[content_type]=json' \
     -f 'events[]=push'
   ```
3. Start gitwatch with the topic URL.
   For a service, put the URL in a file readable only by its user and pass `--ntfy-url-file`, so the URL stays out of the service definition and `ps`.
   Do not set `ntfy_url` in `gitwatch.yaml`: the file sits inside the watched repository, so gitwatch commits it.
   ```sh
   gitwatch watch --remote origin --ntfy-url-file ~/.config/gitwatch/ntfy-url /path/to/repo
   ```

After a dropped connection, gitwatch reconnects with a backoff from 1 to 60 seconds and replays the messages it missed.

</details>

## Installation

<details><summary><b>Nix</b></summary>

A [flake.nix](./flake.nix) is available for Nix:
```nix
inputs = {
  gitwatch-rs.url = "github:croissong/gitwatch-rs";
};
# Reference the package as `inputs.gitwatch-rs.packages.<system>.default`
```

</details>


<details><summary><b>Arch Linux</b></summary>

[AUR package](https://aur.archlinux.org/packages/gitwatch-rs-bin)
```sh
paru -S gitwatch-rs-bin
```

</details>


<details><summary><b>Ubuntu</b></summary>

```sh
# TODO: Add Ubuntu package
```

</details>


<details><summary><b>Cargo</b></summary>

[crates.io](https://crates.io/crates/gitwatch-rs):
```sh
cargo install gitwatch-rs
```

</details>


<details><summary><b>Docker</b></summary>

[ghcr.io/croissong/gitwatch-rs](https://github.com/croissong/gitwatch-rs/pkgs/container/gitwatch-rs):
```sh
docker run -v /path/to/repo:/repo ghcr.io/croissong/gitwatch-rs:latest /repo
```

</details>


<details><summary><b>Binaries</b></summary>

Precompiled binaries are available for Linux and macOS from the [release page](https://github.com/croissong/gitwatch-rs/releases).

</details>
 


### Shell completion

Shell completion scripts for `bash`, `zsh`, `fish` & more can be generated via:
```sh
gitwatch completion <SHELL>
```

## Credits

This is a Rust implementation of the original [gitwatch](https://github.com/gitwatch/gitwatch) bash script.  
 
## Contributing

Contributions are welcome! Feel free to submit a Pull Request.  
See [CONTRIBUTING.md](CONTRIBUTING.md) for development hints.

