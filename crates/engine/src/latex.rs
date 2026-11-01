//! LaTeX → Hwp 수식 스크립트 (EqEdit).
//!
//! Hwp stores an equation as its own script (`{a} over {b}`, `sqrt {x}`, …), which
//! every Hwp version reads. The editor takes LaTeX and writes that script; keywords
//! come from rhwp's EqEdit tables (`rhwp::doclang::eqedit`, the reverse direction)
//! and its equation renderer, which follows Hwp.
//!
//! Letters are written one per token (`a b`, not `ab`): Hwp reads a run of letters
//! as a word, and words like `in`, `to`, `sin` are Hwp commands, while LaTeX means
//! separate variables. Spaces only separate tokens in Hwp, so this draws the same.

/// The result of [`to_hwp`]: the Hwp script, and LaTeX commands that had no Hwp
/// counterpart (written through as plain names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub script: String,
    pub unknown: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Cmd(String),
    Char(char),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    Newline,
    Space,
}

fn tokenize(src: &str) -> Vec<Tok> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        i += 1;
        out.push(match c {
            '\\' => match cs.get(i) {
                Some('\\') => {
                    i += 1;
                    Tok::Newline
                }
                Some(&n) if n.is_ascii_alphabetic() => {
                    let start = i;
                    while i < cs.len() && cs[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    Tok::Cmd(cs[start..i].iter().collect())
                }
                Some(&n) => {
                    i += 1;
                    Tok::Cmd(n.to_string())
                }
                None => Tok::Char('\\'),
            },
            '{' => Tok::Open,
            '}' => Tok::Close,
            '^' => Tok::Sup,
            '_' => Tok::Sub,
            '&' => Tok::Amp,
            c if c.is_whitespace() => Tok::Space,
            c => Tok::Char(c),
        });
    }
    out
}

/// Commands written to Hwp under the same name.
const SAME: &[&str] = &[
    // Greek
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "varepsilon",
    "zeta",
    "eta",
    "theta",
    "vartheta",
    "iota",
    "kappa",
    "lambda",
    "mu",
    "nu",
    "xi",
    "omicron",
    "pi",
    "varpi",
    "rho",
    "varrho",
    "sigma",
    "varsigma",
    "tau",
    "upsilon",
    "phi",
    "varphi",
    "chi",
    "psi",
    "omega",
    // big operators and functions
    "sum",
    "prod",
    "coprod",
    "int",
    "iint",
    "iiint",
    "oint",
    "bigcup",
    "bigcap",
    "lim",
    "sin",
    "cos",
    "tan",
    "cot",
    "sec",
    "csc",
    "sinh",
    "cosh",
    "tanh",
    "arcsin",
    "arccos",
    "arctan",
    "log",
    "ln",
    "exp",
    "max",
    "min",
    "gcd",
    "deg",
    "det",
    // operators, relations, sets, logic
    "times",
    "div",
    "cdot",
    "ast",
    "star",
    "circ",
    "bullet",
    "oplus",
    "otimes",
    "approx",
    "equiv",
    "cong",
    "sim",
    "simeq",
    "propto",
    "in",
    "notin",
    "ni",
    "subset",
    "supset",
    "subseteq",
    "supseteq",
    "cup",
    "cap",
    "emptyset",
    "forall",
    "nabla",
    "partial",
    "therefore",
    "because",
    "cdots",
    "ldots",
    "vdots",
    "ddots",
    "angle",
    "perp",
    "parallel",
    "uparrow",
    "downarrow",
    "aleph",
    "hbar",
    "ell",
    "prime",
    "triangle",
    "diamond",
    "langle",
    "rangle",
    "lfloor",
    "rfloor",
    "lceil",
    "rceil",
];

/// LaTeX command → Hwp token, where the names differ.
fn renamed(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "Gamma" => "GAMMA",
        "Delta" => "DELTA",
        "Theta" => "THETA",
        "Lambda" => "LAMBDA",
        "Xi" => "XI",
        "Pi" => "PI",
        "Sigma" => "SIGMA",
        "Upsilon" => "UPSILON",
        "Phi" => "PHI",
        "Psi" => "PSI",
        "Omega" => "OMEGA",
        "pm" => "+-",
        "mp" => "-+",
        "le" | "leq" => "<=",
        "ge" | "geq" => ">=",
        "ne" | "neq" => "!=",
        "ll" => "<<",
        "gg" => ">>",
        "to" | "rightarrow" => "->",
        "leftarrow" | "gets" => "larrow",
        "leftrightarrow" => "lrarrow",
        "Rightarrow" | "implies" => "Rarrow",
        "Leftarrow" => "Larrow",
        "Leftrightarrow" | "iff" => "LRarrow",
        "infty" => "inf",
        "exists" => "exist",
        "neg" | "lnot" => "neg",
        "land" | "wedge" => "wedge",
        "lor" | "vee" => "vee",
        "dots" => "ldots",
        "{" => "lbrace",
        "}" => "rbrace",
        "|" => "||",
        "%" => "%",
        "$" => "$",
        "_" => "\"_\"",
        "#" => "\"#\"",
        "&" => "\"&\"",
        // spacing: Hwp's ` is a quarter space, ~ a full one
        "," | ":" | ";" => "`",
        "!" => "",
        " " => "~",
        "quad" => "~~",
        "qquad" => "~~~~",
        _ => return None,
    })
}

/// Decorations and font changes that take one argument: LaTeX → Hwp keyword.
fn with_arg(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "sqrt" => "sqrt",
        "bar" => "bar",
        "overline" => "overline",
        "underline" => "under",
        "hat" | "widehat" => "hat",
        "tilde" | "widetilde" => "tilde",
        "vec" | "overrightarrow" => "vec",
        "dot" => "dot",
        "ddot" => "ddot",
        "acute" => "acute",
        "grave" => "grave",
        "check" => "check",
        "mathrm" | "operatorname" => "rm",
        "mathbf" | "textbf" | "boldsymbol" | "bm" => "bold",
        "mathit" | "textit" => "it",
        _ => return None,
    })
}

struct Parser {
    toks: Vec<Tok>,
    i: usize,
    unknown: Vec<String>,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.i).cloned();
        self.i += 1;
        t
    }

    fn skip_space(&mut self) {
        while self.peek() == Some(&Tok::Space) {
            self.i += 1;
        }
    }

    /// Items up to a closing brace (consumed), \end, \right or the end.
    fn seq(&mut self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                None => return Ok(out),
                Some(Tok::Close) => return Ok(out),
                Some(Tok::Cmd(c)) if c == "end" || c == "right" => return Ok(out),
                _ => {}
            }
            let item = self.item()?;
            if !item.is_empty() {
                out.push(item);
            }
        }
    }

    /// `{…}` or one atom, as a Hwp group `{…}`.
    fn group(&mut self) -> Result<String, String> {
        self.skip_space();
        if self.peek() == Some(&Tok::Open) {
            self.i += 1;
            let inner = self.seq()?;
            if self.next() != Some(Tok::Close) {
                return Err("닫는 } 가 없습니다".into());
            }
            Ok(format!("{{{}}}", inner.join(" ")))
        } else {
            let a = self.item()?;
            Ok(format!("{{{a}}}"))
        }
    }

    /// The raw text of a `{…}` argument (for \text, \begin).
    fn raw_arg(&mut self) -> Result<String, String> {
        self.skip_space();
        if self.next() != Some(Tok::Open) {
            return Err("{ 가 필요합니다".into());
        }
        let mut depth = 1;
        let mut s = String::new();
        while let Some(t) = self.next() {
            match t {
                Tok::Open => {
                    depth += 1;
                    s.push('{');
                }
                Tok::Close => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(s);
                    }
                    s.push('}');
                }
                Tok::Char(c) => s.push(c),
                Tok::Space => s.push(' '),
                Tok::Cmd(c) => {
                    s.push('\\');
                    s.push_str(&c);
                }
                Tok::Sup => s.push('^'),
                Tok::Sub => s.push('_'),
                Tok::Amp => s.push('&'),
                Tok::Newline => s.push_str("\\\\"),
            }
        }
        Err("닫는 } 가 없습니다".into())
    }

    /// A delimiter after \left / \right.
    fn delim(&mut self) -> Result<String, String> {
        self.skip_space();
        Ok(match self.next() {
            Some(Tok::Char('.')) => ".".into(),
            Some(Tok::Char(c)) => c.to_string(),
            Some(Tok::Cmd(c)) => match c.as_str() {
                "{" | "lbrace" => "lbrace".into(),
                "}" | "rbrace" => "rbrace".into(),
                "|" | "Vert" => "||".into(),
                "langle" | "rangle" | "lfloor" | "rfloor" | "lceil" | "rceil" => c,
                other => return Err(format!("\\left/\\right 뒤의 \\{other} 는 괄호가 아닙니다")),
            },
            _ => return Err("\\left/\\right 뒤에 괄호가 필요합니다".into()),
        })
    }

    fn item(&mut self) -> Result<String, String> {
        let Some(t) = self.next() else {
            return Ok(String::new());
        };
        Ok(match t {
            Tok::Space => String::new(),
            Tok::Open => {
                let inner = self.seq()?;
                if self.next() != Some(Tok::Close) {
                    return Err("닫는 } 가 없습니다".into());
                }
                format!("{{{}}}", inner.join(" "))
            }
            Tok::Close => return Err("짝이 없는 } 가 있습니다".into()),
            Tok::Sup => format!("^{}", self.group()?),
            Tok::Sub => format!("_{}", self.group()?),
            Tok::Amp => "&".into(),
            Tok::Newline => "#".into(),
            // Hwp's own special characters, written as text
            Tok::Char(c @ ('#' | '~' | '`' | '"')) => format!("\"{c}\""),
            Tok::Char('\'') => "'".into(),
            Tok::Char(c) => c.to_string(),
            Tok::Cmd(c) => self.command(&c)?,
        })
    }

    fn command(&mut self, c: &str) -> Result<String, String> {
        if SAME.contains(&c) {
            return Ok(c.to_owned());
        }
        if let Some(h) = renamed(c) {
            return Ok(h.to_owned());
        }
        Ok(match c {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let (a, b) = (self.group()?, self.group()?);
                format!("{a} over {b}")
            }
            "binom" => {
                let (a, b) = (self.group()?, self.group()?);
                format!("{a} choose {b}")
            }
            "sqrt" => {
                self.skip_space();
                if self.peek() == Some(&Tok::Char('[')) {
                    self.i += 1;
                    let mut n = Vec::new();
                    loop {
                        self.skip_space();
                        match self.peek() {
                            Some(Tok::Char(']')) => {
                                self.i += 1;
                                break;
                            }
                            None => return Err("닫는 ] 가 없습니다".into()),
                            _ => n.push(self.item()?),
                        }
                    }
                    let x = self.group()?;
                    format!("root {{{}}} of {x}", n.join(" "))
                } else {
                    format!("sqrt {}", self.group()?)
                }
            }
            "text" | "textrm" | "mbox" => {
                let s = self.raw_arg()?;
                format!("\"{}\"", s.replace('"', "'"))
            }
            "left" => {
                let open = self.delim()?;
                let body = self.seq()?;
                match self.next() {
                    Some(Tok::Cmd(c)) if c == "right" => {}
                    _ => return Err("\\left 에 맞는 \\right 가 없습니다".into()),
                }
                let close = self.delim()?;
                format!("left {open} {} right {close}", body.join(" "))
            }
            "right" => return Err("\\left 없이 \\right 가 있습니다".into()),
            "begin" => self.environment()?,
            "end" => return Err("짝이 없는 \\end 가 있습니다".into()),
            c => match with_arg(c) {
                Some(h) => format!("{h} {}", self.group()?),
                None => {
                    self.unknown.push(format!("\\{c}"));
                    c.to_owned()
                }
            },
        })
    }

    /// \begin{env} … \end{env}: matrices and cases, rows `#`, cells `&`.
    fn environment(&mut self) -> Result<String, String> {
        let env = self.raw_arg()?;
        let keyword = match env.as_str() {
            "matrix" | "smallmatrix" => "matrix",
            "pmatrix" => "pmatrix",
            "bmatrix" => "bmatrix",
            "vmatrix" => "dmatrix",
            "cases" => "cases",
            "aligned" | "align" | "align*" | "gathered" | "gather" | "split" => "",
            "array" => {
                // the column spec is not needed in Hwp
                let _ = self.raw_arg()?;
                "matrix"
            }
            other => return Err(format!("\\begin{{{other}}} 는 지원하지 않습니다")),
        };
        let body = self.seq()?;
        match self.next() {
            Some(Tok::Cmd(c)) if c == "end" => {
                let end = self.raw_arg()?;
                if end != env {
                    return Err(format!("\\begin{{{env}}} 이 \\end{{{end}}} 로 닫혔습니다"));
                }
            }
            _ => return Err(format!("\\end{{{env}}} 가 없습니다")),
        }
        let body = body.join(" ");
        Ok(if keyword.is_empty() {
            body
        } else {
            format!("{keyword}{{{body}}}")
        })
    }
}

/// LaTeX for an existing Hwp equation script (rhwp's EqEdit converter), to edit it
/// in LaTeX; `None` when rhwp cannot read the script.
pub fn from_hwp(script: &str) -> Option<String> {
    rhwp::doclang::eqedit::convert(script).ok()
}

/// Convert LaTeX math (without `$`) to a Hwp equation script.
pub fn to_hwp(latex: &str) -> Result<Converted, String> {
    let mut p = Parser {
        toks: tokenize(latex.trim().trim_matches('$')),
        i: 0,
        unknown: Vec::new(),
    };
    let mut out = Vec::new();
    loop {
        out.extend(p.seq()?);
        match p.next() {
            None => break,
            Some(Tok::Close) => return Err("짝이 없는 } 가 있습니다".into()),
            Some(Tok::Cmd(c)) if c == "right" => {
                return Err("\\left 없이 \\right 가 있습니다".into())
            }
            Some(Tok::Cmd(c)) => return Err(format!("짝이 없는 \\{c} 가 있습니다")),
            Some(_) => {}
        }
    }
    // every token apart: a run of letters would be one word in Hwp
    Ok(Converted {
        script: out.join(" "),
        unknown: p.unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::to_hwp;

    fn conv(s: &str) -> String {
        to_hwp(s).unwrap().script
    }

    #[test]
    fn fractions_roots_and_scripts() {
        assert_eq!(conv(r"\frac{a}{b}"), "{a} over {b}");
        assert_eq!(conv(r"\sqrt{x^2+1}"), "sqrt {x ^{2} + 1}");
        assert_eq!(conv(r"\sqrt[3]{x}"), "root {3} of {x}");
        assert_eq!(conv(r"x_{i}^{2}"), "x _{i} ^{2}");
        assert_eq!(conv(r"e^{i\pi}"), "e ^{i pi}");
    }

    #[test]
    fn operators_and_greek() {
        assert_eq!(conv(r"\sum_{i=1}^{n} i"), "sum _{i = 1} ^{n} i");
        assert_eq!(
            conv(r"\int_0^\infty e^{-x} dx"),
            "int _{0} ^{inf} e ^{- x} d x"
        );
        assert_eq!(
            conv(r"\lim_{x \to 0} \frac{\sin x}{x} = 1"),
            "lim _{x -> 0} {sin x} over {x} = 1"
        );
        assert_eq!(conv(r"a \le b \ne c \pm d"), "a <= b != c +- d");
        assert_eq!(conv(r"\Gamma(\alpha)"), "GAMMA ( alpha )");
    }

    #[test]
    fn letters_stay_separate_variables() {
        // `in` and `to` are Hwp commands; LaTeX means i·n and t·o
        assert_eq!(conv("in"), "i n");
        assert_eq!(conv("x to y"), "x t o y");
    }

    #[test]
    fn delimiters_matrices_and_text() {
        assert_eq!(
            conv(r"\left( \frac{1}{2} \right)"),
            "left ( {1} over {2} right )"
        );
        assert_eq!(conv(r"\left\{ x \right."), "left lbrace x right .");
        assert_eq!(
            conv(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}"),
            "pmatrix{a & b # c & d}"
        );
        assert_eq!(
            conv(r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{그 외} \end{cases}"),
            "f ( x ) = cases{1 & x > 0 # 0 & \"그 외\"}"
        );
        assert_eq!(conv(r"\mathrm{d}x \quad \vec{v}"), "rm {d} x ~~ vec {v}");
    }

    #[test]
    fn errors_and_unknown_commands() {
        assert!(to_hwp(r"\frac{a}{b").is_err());
        assert!(to_hwp(r"\begin{pmatrix} a").is_err());
        assert!(to_hwp(r"a}").is_err());
        let c = to_hwp(r"\foo x").unwrap();
        assert_eq!(c.unknown, vec![r"\foo".to_string()]);
    }
}
