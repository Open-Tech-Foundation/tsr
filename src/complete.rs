//! Shell completion scripts (`tsr --completions <shell>`).
//!
//! Each script completes flags statically and asks `tsr --list-names` for task
//! names at completion time, so names track the `tasks.toml` (or, configless,
//! the package's scripts) of whatever directory you are in — nothing to
//! regenerate when a task is added.

use crate::error::{Result, TsrError};

/// A shell `tsr` can print a completion script for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    pub fn parse(name: &str) -> Result<Shell> {
        match name {
            "bash" => Ok(Shell::Bash),
            "zsh" => Ok(Shell::Zsh),
            "fish" => Ok(Shell::Fish),
            other => Err(TsrError::runtime(format!(
                "no completions for '{other}' — expected 'bash', 'zsh' or 'fish'"
            ))),
        }
    }

    pub fn script(self) -> &'static str {
        match self {
            Shell::Bash => BASH,
            Shell::Zsh => ZSH,
            Shell::Fish => FISH,
        }
    }
}

/// Flags valid in place of a task name.
const BUILTINS: &str = "--list --config --init --completions --help --version";

/// Flags valid after a task name.
const RUN_OPTIONS: &str = "--since --resume-from --filter --jobs --no-bail --dry-run --expand \
                           --allow-unsafe-env --reporter --reporter-file";

const BASH: &str = r#"# tsr completion for bash — load with:  eval "$(tsr --completions bash)"
_tsr() {
    local cur prev
    # Task names may contain ':', which bash splits words on by default.
    if declare -F _get_comp_words_by_ref >/dev/null; then
        _get_comp_words_by_ref -n : cur prev
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"
    fi
    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$(tsr --list-names 2>/dev/null) @BUILTINS@" -- "$cur"))
    else
        case "$prev" in
            --completions) COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur")) ;;
            --reporter) COMPREPLY=($(compgen -W "human ndjson" -- "$cur")) ;;
            --reporter-file) COMPREPLY=($(compgen -f -- "$cur")) ;;
            --since|--resume-from|--filter|-F|--jobs|-j) COMPREPLY=() ;;
            *) COMPREPLY=($(compgen -W "@RUN_OPTIONS@" -- "$cur")) ;;
        esac
    fi
    if declare -F __ltrim_colon_completions >/dev/null; then
        __ltrim_colon_completions "$cur"
    fi
}
complete -F _tsr tsr
"#;

const ZSH: &str = r#"#compdef tsr
# tsr completion for zsh — load with:  source <(tsr --completions zsh)
_tsr() {
    if (( CURRENT == 2 )); then
        local -a tasks
        tasks=(${(f)"$(tsr --list-names 2>/dev/null)"})
        compadd -a tasks
        compadd -- @BUILTINS@
        return
    fi
    case $words[CURRENT-1] in
        --completions) compadd bash zsh fish ;;
        --reporter) compadd human ndjson ;;
        --reporter-file) _files ;;
        --since|--resume-from|--filter|-F|--jobs|-j) ;;
        *) compadd -- @RUN_OPTIONS@ ;;
    esac
}
if (( $+functions[compdef] )); then
    compdef _tsr tsr
fi
"#;

const FISH: &str = r#"# tsr completion for fish — load with:  tsr --completions fish | source
function __tsr_first_arg
    test (count (commandline -opc)) -eq 1
end
complete -c tsr -f
complete -c tsr -n __tsr_first_arg -a '(tsr --list-names 2>/dev/null)' -d task
complete -c tsr -n __tsr_first_arg -l list -d 'list the tasks'
complete -c tsr -n __tsr_first_arg -l config -d 'edit tasks.toml in a TUI'
complete -c tsr -n __tsr_first_arg -l init -d 'create a starter tasks.toml'
complete -c tsr -n __tsr_first_arg -l completions -x -a 'bash zsh fish' -d 'print a completion script'
complete -c tsr -n __tsr_first_arg -l help -d 'show usage'
complete -c tsr -n __tsr_first_arg -l version -d 'show the version'
complete -c tsr -n 'not __tsr_first_arg' -l since -x -d 'only packages affected since a git ref'
complete -c tsr -n 'not __tsr_first_arg' -l resume-from -x -d 'skip packages ordered before this one'
complete -c tsr -n 'not __tsr_first_arg' -s F -l filter -x -d 'run fan-outs only in this package'
complete -c tsr -n 'not __tsr_first_arg' -s j -l jobs -x -d 'run at most this many commands at once'
complete -c tsr -n 'not __tsr_first_arg' -l no-bail -d 'keep going after a failure'
complete -c tsr -n 'not __tsr_first_arg' -l dry-run -d 'print what would run'
complete -c tsr -n 'not __tsr_first_arg' -l expand -d 'with --dry-run: show expanded commands'
complete -c tsr -n 'not __tsr_first_arg' -l allow-unsafe-env -d 'allow guarded env variables'
complete -c tsr -n 'not __tsr_first_arg' -l reporter -x -a 'human ndjson' -d 'terminal format'
complete -c tsr -n 'not __tsr_first_arg' -l reporter-file -r -F -d 'also write NDJSON here'
"#;

/// The script for `shell`, with the flag lists filled in.
pub fn render(shell: Shell) -> String {
    shell
        .script()
        .replace("@BUILTINS@", BUILTINS)
        .replace("@RUN_OPTIONS@", RUN_OPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_shells_only() {
        assert_eq!(Shell::parse("bash").unwrap(), Shell::Bash);
        assert_eq!(Shell::parse("zsh").unwrap(), Shell::Zsh);
        assert_eq!(Shell::parse("fish").unwrap(), Shell::Fish);
        assert_eq!(Shell::parse("pwsh").unwrap_err().exit_code(), 64);
    }

    #[test]
    fn every_script_asks_tsr_for_task_names_and_has_no_placeholders() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let s = render(shell);
            assert!(s.contains("tsr --list-names"), "{shell:?}");
            assert!(!s.contains('@'), "{shell:?} left a placeholder:\n{s}");
        }
    }

    #[test]
    fn every_run_option_is_offered() {
        // Keep the scripts in step with the parser's option list.
        for opt in RUN_OPTIONS.split_whitespace() {
            for shell in [Shell::Bash, Shell::Zsh] {
                assert!(render(shell).contains(opt), "{shell:?} lacks {opt}");
            }
            let long = opt.trim_start_matches("--");
            assert!(FISH.contains(&format!("-l {long}")), "fish lacks {opt}");
        }
    }
}
