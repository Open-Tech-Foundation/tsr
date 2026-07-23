//! `run`-string parsing and expansion (SPEC §8).
//!
//! A `run` string is lexed into an **AST** — [`Program`] → [`Command`] →
//! [`Word`] → [`Part`] — rather than straight into argv. Keeping the structure
//! is what lets the later expansion pass distinguish text that came from an
//! unquoted literal (where `*` is a glob) from text that came from quotes or a
//! variable (where `*` is just a character).
//!
//! Unsupported constructs (`|` `>` `<` `$(` `` ` `` `&` `(`) are rejected at
//! **load time** with exit code `64` (SPEC §8.2). The resulting plan is one of:
//!
//! - [`RunPlan::Direct`] — every word is static: a single command split into
//!   argv and spawned directly, `execvp`-style (SPEC §8, path 1).
//! - [`RunPlan::Shell`] — the string needs work at run time (variables, globs,
//!   or `&&`/`||`/`;` sequencing), so the mini-shell handles it (SPEC §8.1).
//!
//! Expansion runs in two stages. Variables resolve once per job, against its
//! merged env, so an undefined `$VAR` fails before anything executes. Globs
//! resolve per command, at the moment it runs, so a pattern sees the files an
//! earlier command in the same sequence produced.

use std::path::Path;

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

/// A half-open range of `char` offsets into the original `run` string, used to
/// point a diagnostic at the exact construct that caused it (SPEC §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A `$NAME` / `${NAME}` reference, with the span it occupies in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarRef {
    pub name: String,
    pub span: Span,
}

/// One piece of a word. The variant records *where the text came from*, which
/// decides whether its glob metacharacters are patterns or literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Unquoted literal text. `*`, `?` and `[` here are glob metacharacters.
    Bare(String),
    /// Text from `'...'` or `"..."`. Glob metacharacters are literal.
    Quoted(String),
    /// A variable reference. Its value is always literal — an expanded value is
    /// never rescanned for globs, so a path in `$OUT` can't turn into a pattern.
    Var(VarRef),
}

/// A single argv-word-to-be: the parts that concatenate into one argument
/// (before globbing, which may fan one word out into several).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Word {
    pub parts: Vec<Part>,
}

/// A single command: the words that become its argv.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<Word>,
}

/// A parsed `run` string: a command sequence joined by separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub first: Command,
    pub rest: Vec<(Sep, Command)>,
}

/// The classification of a `run` string (SPEC §8, paths 1 & 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPlan {
    /// Every word is static: argv is known at parse time, spawned directly.
    Direct(Vec<String>),
    /// Needs run-time work (variables, globs, or sequencing).
    Shell(Program),
}

impl RunPlan {
    /// Variable names referenced by this plan (none for a static argv).
    pub fn referenced_vars(&self) -> Vec<VarRef> {
        match self {
            RunPlan::Direct(_) => Vec::new(),
            RunPlan::Shell(p) => p.referenced_vars(),
        }
    }
}

/// One argument after variable expansion, still awaiting globbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// Plain text, used verbatim.
    Literal(String),
    /// A glob pattern, with the literal text to fall back to when it matches
    /// nothing (`sh` behaviour — SPEC §8.1).
    Pattern { pattern: String, literal: String },
}

/// A command whose words have been expanded against the env. Globs are *not*
/// resolved yet — see [`ExpandedCommand::argv`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedCommand {
    pub args: Vec<Arg>,
}

impl ExpandedCommand {
    /// Resolve to concrete argv, expanding globs against `dir`.
    ///
    /// Globbing is deliberately deferred to the moment the command runs, not
    /// done when the plan is built: in `build && rm dist/*.map` the pattern has
    /// to see the files `build` just produced.
    pub fn argv(&self, dir: &Path) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len());
        for arg in &self.args {
            match arg {
                Arg::Literal(s) => argv.push(s.clone()),
                Arg::Pattern { pattern, literal } => match glob_matches(dir, pattern) {
                    // A matching pattern fans one word out into its matches.
                    Some(matches) => argv.extend(matches),
                    None => argv.push(literal.clone()),
                },
            }
        }
        argv
    }
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
    let program = Lexer::new(input).parse_program()?;

    // Direct fast-path: one command whose every word is already a literal. This
    // is a structural property of the AST — quoting alone (`echo 'a b'`) still
    // qualifies, because quotes affect *parsing*, not run-time work.
    if program.rest.is_empty() && program.first.words.iter().all(Word::is_static) {
        let argv: Vec<String> = program.first.words.iter().map(Word::literal).collect();
        return Ok(RunPlan::Direct(argv));
    }
    Ok(RunPlan::Shell(program))
}

impl Word {
    /// True when this word needs no run-time work: no variables, no globs.
    fn is_static(&self) -> bool {
        !self.parts.iter().any(|p| match p {
            Part::Bare(s) => has_glob_meta(s),
            Part::Quoted(_) => false,
            Part::Var(_) => true,
        })
    }

    /// The literal text of a static word (see [`Word::is_static`]).
    fn literal(&self) -> String {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Bare(s) | Part::Quoted(s) => s.as_str(),
                Part::Var(_) => unreachable!("literal() on a word containing a variable"),
            })
            .collect()
    }

    /// Append a part, coalescing adjacent same-kind literals for a tidier AST.
    fn push(&mut self, part: Part) {
        match (self.parts.last_mut(), &part) {
            (Some(Part::Bare(prev)), Part::Bare(next))
            | (Some(Part::Quoted(prev)), Part::Quoted(next)) => prev.push_str(next),
            _ => self.parts.push(part),
        }
    }
}

impl Program {
    /// Expand every word's variables against `lookup`. An undefined variable is
    /// a hard error (SPEC §7.3, exit `64`), raised here — before anything runs —
    /// rather than part-way through a sequence. Globs are resolved later, per
    /// command, by [`ExpandedCommand::argv`].
    pub fn expand(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Result<ExecPlan> {
        let first = expand_command(&self.first, lookup)?;
        let mut rest = Vec::with_capacity(self.rest.len());
        for (sep, cmd) in &self.rest {
            rest.push((*sep, expand_command(cmd, lookup)?));
        }
        Ok(ExecPlan { first, rest })
    }

    /// All variable references in the program, in source order (for load-time
    /// checking and diagnostics).
    pub fn referenced_vars(&self) -> Vec<VarRef> {
        let mut out = Vec::new();
        for cmd in std::iter::once(&self.first).chain(self.rest.iter().map(|(_, c)| c)) {
            for word in &cmd.words {
                for part in &word.parts {
                    if let Part::Var(v) = part {
                        out.push(v.clone());
                    }
                }
            }
        }
        out
    }
}

/// The two renderings of a word: the plain text it expands to, and the glob
/// pattern it represents (with non-`Bare` metacharacters escaped so they stay
/// literal). `is_pattern` says whether the pattern form is meaningful at all.
struct Rendered {
    text: String,
    pattern: String,
    is_pattern: bool,
}

fn render(word: &Word, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Rendered> {
    let mut r = Rendered {
        text: String::new(),
        pattern: String::new(),
        is_pattern: false,
    };
    for part in &word.parts {
        match part {
            Part::Bare(s) => {
                r.text.push_str(s);
                r.pattern.push_str(s);
                r.is_pattern |= has_glob_meta(s);
            }
            Part::Quoted(s) => {
                r.text.push_str(s);
                r.pattern.push_str(&glob::Pattern::escape(s));
            }
            Part::Var(v) => {
                let val = lookup(&v.name).ok_or_else(|| {
                    TsrError::config(format!(
                        "'${}' is not defined in task env, env_file, workspace [env], or .env",
                        v.name
                    ))
                })?;
                r.pattern.push_str(&glob::Pattern::escape(&val));
                r.text.push_str(&val);
            }
        }
    }
    Ok(r)
}

fn expand_command(
    cmd: &Command,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<ExpandedCommand> {
    let mut args = Vec::with_capacity(cmd.words.len());
    for word in &cmd.words {
        let r = render(word, lookup)?;
        args.push(if r.is_pattern {
            Arg::Pattern {
                pattern: r.pattern,
                literal: r.text,
            }
        } else {
            Arg::Literal(r.text)
        });
    }
    Ok(ExpandedCommand { args })
}

/// Characters that make a bare word a glob pattern.
fn has_glob_meta(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// Match `pattern` against the filesystem, relative to `dir`. Returns `None`
/// when the pattern is unparseable or matches nothing; otherwise the matches,
/// sorted, and relative to `dir` unless the pattern was absolute.
///
/// Matching deliberately mirrors `sh`: `*` does not cross a path separator and
/// does not match a leading dot. Case sensitivity follows the platform.
fn glob_matches(dir: &Path, pattern: &str) -> Option<Vec<String>> {
    let opts = glob::MatchOptions {
        case_sensitive: !cfg!(windows),
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };

    let absolute = Path::new(pattern).is_absolute();
    let full = if absolute {
        pattern.to_string()
    } else {
        // The base directory is fixed text, not part of the pattern, so escape
        // any metacharacters a path component happens to contain.
        let base = glob::Pattern::escape(&dir.to_string_lossy());
        format!("{}/{}", base.trim_end_matches(['/', '\\']), pattern)
    };

    let mut out: Vec<String> = glob::glob_with(&full, opts)
        .ok()?
        .filter_map(|r| r.ok())
        .map(|p| {
            let rel = if absolute {
                None
            } else {
                p.strip_prefix(dir).ok()
            };
            rel.unwrap_or(&p).to_string_lossy().into_owned()
        })
        .collect();
    if out.is_empty() {
        return None;
    }
    out.sort();
    Some(out)
}

impl Sep {
    /// Whether the command after this separator runs, given the previous
    /// command's exit code (SPEC §8.1). This is the single definition of the
    /// sequencing rule; [`exec`](crate::exec) drives it from its own
    /// abort-aware loop so a fail-fast can interrupt a sequence mid-way.
    pub fn proceeds(self, prev: i32) -> bool {
        match self {
            Sep::And => prev == 0,
            Sep::Or => prev != 0,
            Sep::Semi => true,
        }
    }
}

/// Render a source span as a caret line under `run = "<src>"`, matching the
/// diagnostic layout in SPEC §7.3.
pub fn caret(src: &str, span: Span) -> String {
    // Width of the `  run = "` prefix the line is printed behind.
    const PREFIX: usize = 9;
    let width = span.end.saturating_sub(span.start).max(1);
    format!(
        "  run = \"{src}\"\n{pad}{carets}",
        pad = " ".repeat(PREFIX + span.start),
        carets = "^".repeat(width),
    )
}

/// Quote-aware lexer that parses a `run` string into a [`Program`] and rejects
/// unsupported constructs.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Lexer {
        Lexer {
            chars: input.chars().collect(),
            pos: 0,
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
            commands.push(self.parse_command()?);
            match self.parse_separator()? {
                Some(sep) => seps.push(sep),
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
        let mut words: Vec<Word> = Vec::new();
        let mut cur = Word::default();
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
                    word_started = true;
                    self.lex_single_quote(&mut cur)?;
                }
                Some('"') => {
                    word_started = true;
                    self.lex_double_quote(&mut cur)?;
                }
                Some('$') => {
                    word_started = true;
                    match self.lex_dollar()? {
                        Some(var) => cur.push(Part::Var(var)),
                        None => cur.push(Part::Bare("$".into())),
                    }
                }
                Some(c) => {
                    reject_unsupported(c)?;
                    self.bump();
                    word_started = true;
                    cur.push(Part::Bare(c.to_string()));
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

    /// `'...'` — everything literal, no expansion, no globbing (SPEC §8.1).
    fn lex_single_quote(&mut self, cur: &mut Word) -> Result<()> {
        self.bump(); // opening quote
        let mut lit = String::new();
        loop {
            match self.bump() {
                Some('\'') => break,
                Some(c) => lit.push(c),
                None => {
                    return Err(TsrError::config("'run' string: unterminated single quote"));
                }
            }
        }
        cur.push(Part::Quoted(lit));
        Ok(())
    }

    /// `"..."` — literal text with `$VAR`/`${VAR}` expansion (SPEC §8.1).
    /// Command substitution and backticks remain rejected inside double quotes.
    fn lex_double_quote(&mut self, cur: &mut Word) -> Result<()> {
        self.bump(); // opening quote
        loop {
            match self.peek() {
                Some('"') => {
                    self.bump();
                    break;
                }
                Some('$') => match self.lex_dollar()? {
                    Some(var) => cur.push(Part::Var(var)),
                    None => cur.push(Part::Quoted("$".into())),
                },
                Some('`') => return Err(unsupported_substitution()),
                Some(c) => {
                    self.bump();
                    cur.push(Part::Quoted(c.to_string()));
                }
                None => {
                    return Err(TsrError::config("'run' string: unterminated double quote"));
                }
            }
        }
        Ok(())
    }

    /// Parse a `$`-introduced token into a [`VarRef`], or `None` when the `$` is
    /// just a literal dollar sign. Rejects `$(...)` substitution (SPEC §8.2).
    fn lex_dollar(&mut self) -> Result<Option<VarRef>> {
        let start = self.pos;
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
                            return Err(TsrError::config("'run' string: unterminated '${...}'"));
                        }
                    }
                }
                validate_var_name(&name)?;
                Ok(Some(VarRef {
                    name,
                    span: Span {
                        start,
                        end: self.pos,
                    },
                }))
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
                Ok(Some(VarRef {
                    name,
                    span: Span {
                        start,
                        end: self.pos,
                    },
                }))
            }
            // A `$` not introducing a variable is a literal dollar sign.
            _ => Ok(None),
        }
    }
}

/// A `${...}` body must be a plain variable name. Anything else is a shell
/// parameter expansion the mini-shell does not implement, and saying so beats
/// failing later with "'${VAR:-x}' is not defined".
fn validate_var_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(TsrError::config("'run' string: empty '${}' variable"));
    }
    let valid = !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric());
    if valid {
        return Ok(());
    }
    Err(TsrError::config(format!(
        "'run' string: '${{{name}}}' is not a plain variable name — parameter \
         expansion (':-', ':+', '#', …) is unsupported; set a default in [env] \
         or use a script file"
    )))
}

/// Reject an unsupported metacharacter with a message pointing at the escape
/// hatch (SPEC §8.2 table).
fn reject_unsupported(c: char) -> Result<()> {
    match c {
        '|' => Err(unsupported_msg(
            '|',
            "pipe",
            "use `delegate` or a script file",
        )),
        '>' | '<' => Err(unsupported_msg(c, "redirection", "use a script file")),
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// Fully resolve a `run` string to the argv of each command in its
    /// sequence, whichever plan it classified as.
    fn expand_in(input: &str, env: &[(&str, &str)], dir: &Path) -> Vec<Vec<String>> {
        let map: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        match parse(input).unwrap() {
            RunPlan::Direct(argv) => vec![argv],
            RunPlan::Shell(p) => {
                let plan = p.expand(&|k| map.get(k).cloned()).unwrap();
                std::iter::once(&plan.first)
                    .chain(plan.rest.iter().map(|(_, c)| c))
                    .map(|c| c.argv(dir))
                    .collect()
            }
        }
    }

    fn expand_argv(input: &str, env: &[(&str, &str)]) -> Vec<Vec<String>> {
        expand_in(input, env, Path::new("/nonexistent-tsr-glob-base"))
    }

    /// Drive a sequence the way [`exec`](crate::exec) does, to exercise the
    /// `&&`/`||`/`;` rule in [`Sep::proceeds`] in isolation.
    fn drive(plan: &ExecPlan, run: &mut dyn FnMut(&[String]) -> i32) -> (Vec<String>, i32) {
        let dir = Path::new(".");
        let mut ran = Vec::new();
        let argv = plan.first.argv(dir);
        ran.push(argv[0].clone());
        let mut code = run(&argv);
        for (sep, cmd) in &plan.rest {
            if !sep.proceeds(code) {
                continue;
            }
            let argv = cmd.argv(dir);
            ran.push(argv[0].clone());
            code = run(&argv);
        }
        (ran, code)
    }

    /// A scratch directory with `files` created inside it.
    fn scratch(files: &[&str]) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-shell-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for f in files {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, "").unwrap();
        }
        dir
    }

    // --- classification ---

    #[test]
    fn plain_string_is_direct_spawn() {
        assert_eq!(direct("vite --host"), vec!["vite", "--host"]);
        assert_eq!(direct("  cargo   build "), vec!["cargo", "build"]);
    }

    #[test]
    fn quoting_alone_stays_direct() {
        // Quotes affect parsing, not run-time work, so the word is still static.
        assert_eq!(direct("echo 'hello world'"), vec!["echo", "hello world"]);
        assert_eq!(direct("echo \"a b\""), vec!["echo", "a b"]);
        assert_eq!(direct("echo ''"), vec!["echo", ""]);
    }

    #[test]
    fn vars_globs_and_operators_need_the_shell() {
        assert!(matches!(parse("a $B").unwrap(), RunPlan::Shell(_)));
        assert!(matches!(parse("rm dist/*").unwrap(), RunPlan::Shell(_)));
        assert!(matches!(parse("a && b").unwrap(), RunPlan::Shell(_)));
    }

    // --- quoting & variables ---

    #[test]
    fn single_quotes_are_literal() {
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
        let err = shell("deploy $MISSING").expand(&|_| None).unwrap_err();
        assert!(matches!(err, TsrError::Config(_)));
        assert!(err.to_string().contains("$MISSING"));
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn parameter_expansion_is_a_targeted_error() {
        let err = parse("deploy ${TARGET:-prod}").unwrap_err();
        assert!(err.to_string().contains("parameter expansion"), "{err}");
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn var_spans_point_at_the_reference() {
        let vars = shell("deploy --target $TARGET").referenced_vars();
        assert_eq!(vars.len(), 1);
        let span = vars[0].span;
        let src = "deploy --target $TARGET";
        let text: String = src
            .chars()
            .skip(span.start)
            .take(span.end - span.start)
            .collect();
        assert_eq!(text, "$TARGET");
    }

    #[test]
    fn referenced_vars_collected() {
        let mut vars: Vec<String> = shell("a $X && b ${Y}")
            .referenced_vars()
            .into_iter()
            .map(|v| v.name)
            .collect();
        vars.sort();
        assert_eq!(vars, vec!["X", "Y"]);
    }

    // --- globbing ---

    #[test]
    fn glob_expands_relative_to_the_task_dir() {
        let dir = scratch(&["dist/a.js", "dist/b.js", "src/keep.rs"]);
        assert_eq!(
            expand_in("rm dist/*", &[], &dir),
            vec![vec!["rm", "dist/a.js", "dist/b.js"]]
        );
    }

    #[test]
    fn glob_matches_are_sorted_and_fan_out() {
        let dir = scratch(&["c.txt", "a.txt", "b.txt"]);
        assert_eq!(
            expand_in("rm *.txt", &[], &dir),
            vec![vec!["rm", "a.txt", "b.txt", "c.txt"]]
        );
    }

    #[test]
    fn unmatched_glob_stays_literal() {
        let dir = scratch(&[]);
        assert_eq!(
            expand_in("rm dist/*", &[], &dir),
            vec![vec!["rm", "dist/*"]]
        );
    }

    #[test]
    fn glob_does_not_cross_separators_or_match_dotfiles() {
        let dir = scratch(&["a.js", "nested/b.js", ".hidden.js"]);
        assert_eq!(expand_in("rm *.js", &[], &dir), vec![vec!["rm", "a.js"]]);
    }

    #[test]
    fn quoted_metachars_are_not_globs() {
        let dir = scratch(&["a.txt"]);
        // The pattern is quoted, so it stays a literal argument.
        assert_eq!(
            expand_in("echo '*.txt'", &[], &dir),
            vec![vec!["echo", "*.txt"]]
        );
    }

    #[test]
    fn expanded_variables_are_not_rescanned_for_globs() {
        let dir = scratch(&["a.txt"]);
        assert_eq!(
            expand_in("echo $P", &[("P", "*.txt")], &dir),
            vec![vec!["echo", "*.txt"]]
        );
    }

    #[test]
    fn var_and_glob_combine_in_one_word() {
        let dir = scratch(&["build/one.js", "build/two.js"]);
        assert_eq!(
            expand_in("rm $OUT/*.js", &[("OUT", "build")], &dir),
            vec![vec!["rm", "build/one.js", "build/two.js"]]
        );
    }

    #[test]
    fn question_mark_and_class_patterns_work() {
        let dir = scratch(&["a1.log", "a2.log", "bb.log"]);
        assert_eq!(
            expand_in("rm a?.log", &[], &dir),
            vec![vec!["rm", "a1.log", "a2.log"]]
        );
        assert_eq!(
            expand_in("rm [ab]b.log", &[], &dir),
            vec![vec!["rm", "bb.log"]]
        );
    }

    #[test]
    fn unparseable_pattern_stays_literal() {
        let dir = scratch(&[]);
        // An unclosed class is not a valid pattern; keep the word as typed.
        assert_eq!(expand_in("echo a[b", &[], &dir), vec![vec!["echo", "a[b"]]);
    }

    // --- sequencing ---

    #[test]
    fn sep_proceeds_rule() {
        assert!(Sep::And.proceeds(0) && !Sep::And.proceeds(1));
        assert!(!Sep::Or.proceeds(0) && Sep::Or.proceeds(1));
        assert!(Sep::Semi.proceeds(0) && Sep::Semi.proceeds(7));
    }

    #[test]
    fn sequencing_and_semantics() {
        let plan = shell("a && b").expand(&|_| None).unwrap();
        let (ran, code) = drive(&plan, &mut |argv| if argv[0] == "a" { 1 } else { 0 });
        assert_eq!(ran, vec!["a"]); // b skipped
        assert_eq!(code, 1);
    }

    #[test]
    fn sequencing_or_semantics() {
        let plan = shell("a || b").expand(&|_| None).unwrap();
        let (ran, code) = drive(&plan, &mut |argv| if argv[0] == "a" { 1 } else { 0 });
        assert_eq!(ran, vec!["a", "b"]);
        assert_eq!(code, 0);
    }

    #[test]
    fn sequencing_semicolon_always_runs() {
        let plan = shell("a ; b").expand(&|_| None).unwrap();
        let (ran, code) = drive(&plan, &mut |argv| if argv[0] == "a" { 3 } else { 0 });
        assert_eq!(ran, vec!["a", "b"]);
        assert_eq!(code, 0);
    }

    // --- rejection ---

    #[test]
    fn rejects_pipe() {
        assert!(
            parse("cat x | grep y")
                .unwrap_err()
                .to_string()
                .contains("pipe")
        );
    }

    #[test]
    fn rejects_redirection() {
        assert!(
            parse("echo x > file")
                .unwrap_err()
                .to_string()
                .contains("redirection")
        );
        assert!(parse("cmd 2>&1").is_err());
    }

    #[test]
    fn rejects_command_substitution() {
        assert!(
            parse("echo $(date)")
                .unwrap_err()
                .to_string()
                .contains("substitution")
        );
        assert!(parse("echo `date`").is_err());
    }

    #[test]
    fn rejects_single_ampersand_and_pipe() {
        assert!(parse("sleep 1 &").is_err());
        assert!(parse("a | b").is_err());
    }

    #[test]
    fn metachar_inside_quotes_is_not_rejected() {
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
    fn caret_underlines_the_span() {
        let src = "deploy --target $TARGET";
        let span = shell(src).referenced_vars()[0].span;
        let out = caret(src, span);
        let (source_line, caret_line) = out.split_once('\n').unwrap();
        assert_eq!(source_line, "  run = \"deploy --target $TARGET\"");
        // The carets must sit exactly under `$TARGET` in the line above.
        assert_eq!(caret_line.len(), source_line.len() - 1);
        assert_eq!(caret_line.trim_start(), "^".repeat("$TARGET".len()));
        assert_eq!(
            source_line.find("$TARGET").unwrap(),
            caret_line.find('^').unwrap()
        );
    }
}
