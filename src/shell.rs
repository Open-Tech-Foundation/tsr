//! `run`-string execution model (SPEC §8).
//!
//! A `run` string is lexed, quote-aware, into a [`Program`]. During lexing,
//! unsupported constructs (`|` `>` `<` `*` `?` `[` `$(` `` ` `` `&` `(`) are
//! rejected at **load time** with exit code `64` (SPEC §8.2). The resulting plan
//! is one of:
//!
//! - [`RunPlan::Direct`] — no shell features at all: a single command split into
//!   argv and spawned directly, `execvp`-style (SPEC §8, path 1).
//! - [`RunPlan::Shell`] — supported metacharacters present (`$VAR`, `&& || ;`,
//!   quoting): the mini-shell expands variables (SPEC §7.3) and sequences
//!   commands with correct exit-code semantics (SPEC §8.1).
//!
//! Detection order (SPEC §8.4): the lexer always runs first, so a metachar-free
//! string is classified `Direct` and never touches variable/operator handling.

use crate::error::{Result, TsrError};

/// The separator preceding a command in a sequence (SPEC §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sep {
    /// `&&` — run the next command only if the previous succeeded (exit 0).
    And,
    /// `||` — run the next command only if the previous failed (exit ≠ 0).
    Or,
    /// `;` — always run the next command.
    Semi,
}

/// A piece of a word: either literal text or a variable to expand.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fragment {
    Literal(String),
    /// `$NAME` / `${NAME}` — expanded against the merged env (SPEC §7.3).
    Var(String),
}

/// A single command: a sequence of words (argv-to-be), each built from
/// fragments so variable expansion can be deferred until the env is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Command {
    words: Vec<Vec<Fragment>>,
}

/// A parsed `run` string: a command sequence joined by separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    first: Command,
    rest: Vec<(Sep, Command)>,
}

/// The classification of a `run` string (SPEC §8, paths 1 & 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPlan {
    /// No shell features: static argv, spawned directly.
    Direct(Vec<String>),
    /// Supported metacharacters present: handled by the mini-shell.
    Shell(Program),
}

/// A command whose words have been expanded against the env, ready to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedCommand {
    pub argv: Vec<String>,
}

/// An expanded command sequence, ready for the mini-shell to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecPlan {
    pub first: ExpandedCommand,
    pub rest: Vec<(Sep, ExpandedCommand)>,
}

/// Parse and classify a `run` string. Rejects unsupported metacharacters at load
/// time (exit `64`).
pub fn parse(input: &str) -> Result<RunPlan> {
    let mut lexer = Lexer::new(input);
    let program = lexer.parse_program()?;

    // Direct fast-path: a single command, no operators, no quotes, no vars.
    if !lexer.saw_shell_feature && program.rest.is_empty() {
        let argv: Vec<String> = program
            .first
            .words
            .iter()
            .map(|w| {
                w.iter()
                    .map(|f| match f {
                        Fragment::Literal(s) => s.as_str(),
                        Fragment::Var(_) => unreachable!("no vars without shell feature"),
                    })
                    .collect::<String>()
            })
            .collect();
        if argv.is_empty() {
            return Err(TsrError::config("'run' string is empty"));
        }
        return Ok(RunPlan::Direct(argv));
    }
    Ok(RunPlan::Shell(program))
}

impl Program {
    /// Expand every word against `lookup`, which resolves a variable name to its
    /// value. An undefined variable is a hard error (SPEC §7.3, exit `64`).
    pub fn expand(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Result<ExecPlan> {
        let first = expand_command(&self.first, lookup)?;
        let mut rest = Vec::with_capacity(self.rest.len());
        for (sep, cmd) in &self.rest {
            rest.push((*sep, expand_command(cmd, lookup)?));
        }
        Ok(ExecPlan { first, rest })
    }

    /// All variable names referenced by the program (for load-time checking).
    pub fn referenced_vars(&self) -> Vec<String> {
        let mut out = Vec::new();
        for cmd in std::iter::once(&self.first).chain(self.rest.iter().map(|(_, c)| c)) {
            for word in &cmd.words {
                for frag in word {
                    if let Fragment::Var(name) = frag {
                        out.push(name.clone());
                    }
                }
            }
        }
        out
    }
}

fn expand_command(
    cmd: &Command,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<ExpandedCommand> {
    let mut argv = Vec::with_capacity(cmd.words.len());
    for word in &cmd.words {
        let mut s = String::new();
        for frag in word {
            match frag {
                Fragment::Literal(lit) => s.push_str(lit),
                Fragment::Var(name) => {
                    let val = lookup(name).ok_or_else(|| {
                        TsrError::config(format!(
                            "'${name}' is not defined in task env, workspace [env], or .env"
                        ))
                    })?;
                    s.push_str(&val);
                }
            }
        }
        argv.push(s);
    }
    Ok(ExpandedCommand { argv })
}

impl ExecPlan {
    /// Execute the sequence, applying `&&`/`||`/`;` semantics, using `spawn` to
    /// run each command and yield its exit code. Returns the sequence's exit
    /// code: the last command actually executed (SPEC §8.1).
    pub fn run(&self, spawn: &mut dyn FnMut(&[String]) -> i32) -> i32 {
        let mut code = spawn(&self.first.argv);
        for (sep, cmd) in &self.rest {
            let should_run = match sep {
                Sep::And => code == 0,
                Sep::Or => code != 0,
                Sep::Semi => true,
            };
            if should_run {
                code = spawn(&cmd.argv);
            }
        }
        code
    }
}

/// Quote-aware lexer that parses a `run` string into a [`Program`] and rejects
/// unsupported constructs.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    /// Set when any mini-shell feature (quote, `$`, or operator) is seen.
    saw_shell_feature: bool,
}

impl Lexer {
    fn new(input: &str) -> Lexer {
        Lexer {
            chars: input.chars().collect(),
            pos: 0,
            saw_shell_feature: false,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        self.pos += 1;
        c
    }

    fn parse_program(&mut self) -> Result<Program> {
        let mut commands: Vec<Command> = Vec::new();
        let mut seps: Vec<Sep> = Vec::new();

        loop {
            let cmd = self.parse_command()?;
            commands.push(cmd);
            match self.parse_separator()? {
                Some(sep) => {
                    self.saw_shell_feature = true;
                    seps.push(sep);
                }
                None => break,
            }
        }

        let mut iter = commands.into_iter();
        let first = iter.next().expect("at least one command");
        // A trailing/leading operator would leave an empty command.
        if first.words.is_empty() {
            return Err(TsrError::config("'run' string: missing command"));
        }
        let mut rest = Vec::new();
        for (sep, cmd) in seps.into_iter().zip(iter) {
            if cmd.words.is_empty() {
                return Err(TsrError::config(
                    "'run' string: missing command around '&&'/'||'/';'",
                ));
            }
            rest.push((sep, cmd));
        }
        Ok(Program { first, rest })
    }

    /// Parse a single command up to the next separator or end of input.
    fn parse_command(&mut self) -> Result<Command> {
        let mut words: Vec<Vec<Fragment>> = Vec::new();
        let mut cur: Vec<Fragment> = Vec::new();
        let mut word_started = false;

        loop {
            match self.peek() {
                None => break,
                Some(c) if c.is_whitespace() => {
                    self.bump();
                    if word_started {
                        words.push(std::mem::take(&mut cur));
                        word_started = false;
                    }
                }
                // Separators end the command; handled by parse_separator.
                Some(';') => break,
                Some('&') if self.peek2() == Some('&') => break,
                Some('|') if self.peek2() == Some('|') => break,
                Some('\'') => {
                    self.saw_shell_feature = true;
                    word_started = true;
                    self.lex_single_quote(&mut cur)?;
                }
                Some('"') => {
                    self.saw_shell_feature = true;
                    word_started = true;
                    self.lex_double_quote(&mut cur)?;
                }
                Some('$') => {
                    self.saw_shell_feature = true;
                    word_started = true;
                    let frag = self.lex_dollar()?;
                    push_fragment(&mut cur, frag);
                }
                Some(c) => {
                    reject_unsupported(c)?;
                    self.bump();
                    word_started = true;
                    push_fragment(&mut cur, Fragment::Literal(c.to_string()));
                }
            }
        }
        if word_started {
            words.push(cur);
        }
        Ok(Command { words })
    }

    /// After a command, consume a separator if present.
    fn parse_separator(&mut self) -> Result<Option<Sep>> {
        match self.peek() {
            Some(';') => {
                self.bump();
                Ok(Some(Sep::Semi))
            }
            Some('&') if self.peek2() == Some('&') => {
                self.bump();
                self.bump();
                Ok(Some(Sep::And))
            }
            Some('|') if self.peek2() == Some('|') => {
                self.bump();
                self.bump();
                Ok(Some(Sep::Or))
            }
            None => Ok(None),
            // A bare `&` or `|` here is unsupported; surface the precise error.
            Some(c) => {
                reject_unsupported(c)?;
                Ok(None)
            }
        }
    }

    /// `'...'` — everything literal, no expansion (SPEC §8.1).
    fn lex_single_quote(&mut self, cur: &mut Vec<Fragment>) -> Result<()> {
        self.bump(); // opening quote
        let mut lit = String::new();
        loop {
            match self.bump() {
                Some('\'') => break,
                Some(c) => lit.push(c),
                None => {
                    return Err(TsrError::config(
                        "'run' string: unterminated single quote",
                    ));
                }
            }
        }
        push_fragment(cur, Fragment::Literal(lit));
        Ok(())
    }

    /// `"..."` — literal text with `$VAR`/`${VAR}` expansion (SPEC §8.1).
    /// Command substitution and backticks remain rejected inside double quotes.
    fn lex_double_quote(&mut self, cur: &mut Vec<Fragment>) -> Result<()> {
        self.bump(); // opening quote
        loop {
            match self.peek() {
                Some('"') => {
                    self.bump();
                    break;
                }
                Some('$') => {
                    let frag = self.lex_dollar()?;
                    push_fragment(cur, frag);
                }
                Some('`') => {
                    return Err(unsupported('`'));
                }
                Some(c) => {
                    self.bump();
                    push_fragment(cur, Fragment::Literal(c.to_string()));
                }
                None => {
                    return Err(TsrError::config(
                        "'run' string: unterminated double quote",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Parse a `$`-introduced token: `${NAME}`, `$NAME`, or a literal `$`.
    /// Rejects `$(...)` command substitution (SPEC §8.2).
    fn lex_dollar(&mut self) -> Result<Fragment> {
        self.bump(); // consume '$'
        match self.peek() {
            Some('(') => Err(unsupported_substitution()),
            Some('{') => {
                self.bump();
                let mut name = String::new();
                loop {
                    match self.bump() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => {
                            return Err(TsrError::config(
                                "'run' string: unterminated '${...}'",
                            ));
                        }
                    }
                }
                if name.is_empty() {
                    return Err(TsrError::config("'run' string: empty '${}' variable"));
                }
                Ok(Fragment::Var(name))
            }
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {
                let mut name = String::new();
                while let Some(c) = self.peek() {
                    if c == '_' || c.is_ascii_alphanumeric() {
                        name.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                Ok(Fragment::Var(name))
            }
            // A `$` not introducing a variable is a literal dollar sign.
            _ => Ok(Fragment::Literal("$".into())),
        }
    }
}

/// Append a fragment, coalescing adjacent literals for a tidier AST.
fn push_fragment(word: &mut Vec<Fragment>, frag: Fragment) {
    if let (Some(Fragment::Literal(prev)), Fragment::Literal(next)) = (word.last_mut(), &frag) {
        prev.push_str(next);
    } else {
        word.push(frag);
    }
}

/// Reject an unsupported metacharacter with a message pointing at the escape
/// hatch (SPEC §8.2 table).
fn reject_unsupported(c: char) -> Result<()> {
    match c {
        '|' => Err(unsupported_msg('|', "pipe", "use `delegate` or a script file")),
        '>' | '<' => Err(unsupported_msg(
            c,
            "redirection",
            "use a script file",
        )),
        '*' | '?' | '[' => Err(unsupported_msg(
            c,
            "glob",
            "pass an explicit path",
        )),
        '`' => Err(unsupported_substitution()),
        '&' => Err(unsupported_msg(
            '&',
            "background/control operator",
            "use `&&`, or `delegate` for real shell control",
        )),
        '(' | ')' => Err(unsupported_msg(
            c,
            "subshell",
            "use `delegate` or a script file",
        )),
        _ => Ok(()),
    }
}

fn unsupported(c: char) -> TsrError {
    unsupported_msg(c, "metacharacter", "use `delegate` or a script file")
}

fn unsupported_substitution() -> TsrError {
    TsrError::config(
        "'run' string uses command substitution ('$(...)' or backticks), \
         which is unsupported — use a script file",
    )
}

fn unsupported_msg(c: char, kind: &str, hint: &str) -> TsrError {
    TsrError::config(format!(
        "'run' string uses '{c}' ({kind}), which is unsupported — {hint}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn direct(input: &str) -> Vec<String> {
        match parse(input).unwrap() {
            RunPlan::Direct(argv) => argv,
            other => panic!("expected Direct, got {other:?}"),
        }
    }

    fn shell(input: &str) -> Program {
        match parse(input).unwrap() {
            RunPlan::Shell(p) => p,
            other => panic!("expected Shell, got {other:?}"),
        }
    }

    fn expand_argv(input: &str, env: &[(&str, &str)]) -> Vec<Vec<String>> {
        let map: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let plan = shell(input)
            .expand(&|k| map.get(k).cloned())
            .unwrap();
        std::iter::once(plan.first.clone())
            .chain(plan.rest.iter().map(|(_, c)| c.clone()))
            .map(|c| c.argv)
            .collect()
    }

    #[test]
    fn plain_string_is_direct_spawn() {
        assert_eq!(direct("vite --host"), vec!["vite", "--host"]);
        assert_eq!(direct("  cargo   build "), vec!["cargo", "build"]);
    }

    #[test]
    fn quotes_group_words() {
        assert_eq!(expand_argv("echo 'hello world'", &[]), vec![vec!["echo", "hello world"]]);
        assert_eq!(expand_argv("echo \"a b\"", &[]), vec![vec!["echo", "a b"]]);
    }

    #[test]
    fn single_quotes_are_literal() {
        // '$VAR' inside single quotes is not expanded.
        assert_eq!(
            expand_argv("echo '$VAR'", &[("VAR", "x")]),
            vec![vec!["echo", "$VAR"]]
        );
    }

    #[test]
    fn double_quotes_expand() {
        assert_eq!(
            expand_argv("echo \"hi $NAME\"", &[("NAME", "sam")]),
            vec![vec!["echo", "hi sam"]]
        );
    }

    #[test]
    fn expands_bare_and_braced_vars() {
        assert_eq!(
            expand_argv("deploy --target $TARGET", &[("TARGET", "prod")]),
            vec![vec!["deploy", "--target", "prod"]]
        );
        assert_eq!(
            expand_argv("deploy ${TARGET}x", &[("TARGET", "prod")]),
            vec![vec!["deploy", "prodx"]]
        );
    }

    #[test]
    fn undefined_var_is_hard_error() {
        let err = shell("deploy $MISSING")
            .expand(&|_| None)
            .unwrap_err();
        assert!(matches!(err, TsrError::Config(_)));
        assert!(err.to_string().contains("$MISSING"));
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn sequencing_and_semantics() {
        // && runs second only on success; the runner short-circuits on failure.
        let plan = shell("a && b").expand(&|_| None).unwrap();
        let mut ran: Vec<String> = Vec::new();
        let code = plan.run(&mut |argv| {
            ran.push(argv[0].clone());
            if argv[0] == "a" { 1 } else { 0 }
        });
        assert_eq!(ran, vec!["a"]); // b skipped
        assert_eq!(code, 1);
    }

    #[test]
    fn sequencing_or_semantics() {
        let plan = shell("a || b").expand(&|_| None).unwrap();
        let mut ran: Vec<String> = Vec::new();
        let code = plan.run(&mut |argv| {
            ran.push(argv[0].clone());
            if argv[0] == "a" { 1 } else { 0 }
        });
        assert_eq!(ran, vec!["a", "b"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn sequencing_semicolon_always_runs() {
        let plan = shell("a ; b").expand(&|_| None).unwrap();
        let mut ran: Vec<String> = Vec::new();
        let code = plan.run(&mut |argv| {
            ran.push(argv[0].clone());
            if argv[0] == "a" { 3 } else { 0 }
        });
        assert_eq!(ran, vec!["a", "b"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn rejects_pipe() {
        assert!(parse("cat x | grep y").unwrap_err().to_string().contains("pipe"));
    }

    #[test]
    fn rejects_redirection() {
        assert!(parse("echo x > file").unwrap_err().to_string().contains("redirection"));
        assert!(parse("cmd 2>&1").is_err());
    }

    #[test]
    fn rejects_glob() {
        assert!(parse("rm *.tmp").unwrap_err().to_string().contains("glob"));
        assert!(parse("ls a?b").is_err());
    }

    #[test]
    fn rejects_command_substitution() {
        assert!(parse("echo $(date)").unwrap_err().to_string().contains("substitution"));
        assert!(parse("echo `date`").is_err());
    }

    #[test]
    fn rejects_single_ampersand_and_pipe() {
        assert!(parse("sleep 1 &").is_err());
        assert!(parse("a | b").is_err());
    }

    #[test]
    fn metachar_inside_quotes_is_not_rejected() {
        // A pipe inside single quotes is a literal, not a rejected pipe.
        assert_eq!(
            expand_argv("echo 'a | b'", &[]),
            vec![vec!["echo", "a | b"]]
        );
        assert_eq!(expand_argv("echo '> x'", &[]), vec![vec!["echo", "> x"]]);
    }

    #[test]
    fn rejects_unterminated_quote() {
        assert!(parse("echo 'oops").is_err());
        assert!(parse("echo \"oops").is_err());
    }

    #[test]
    fn referenced_vars_collected() {
        let mut vars = shell("a $X && b ${Y}").referenced_vars();
        vars.sort();
        assert_eq!(vars, vec!["X", "Y"]);
    }
}
