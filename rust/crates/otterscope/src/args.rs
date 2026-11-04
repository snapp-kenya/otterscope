//! Go-`flag`-compatible argument parsing.
//!
//! CLI compatibility is part of the product: operators script
//! `otterscope serve -listen :8317 -db /data/otterscope.db`, and every existing
//! wrapper, unit file and Compose command must keep working after the port. So
//! this parser reproduces Go's `flag` package rules rather than clap's:
//!
//! - one or two leading dashes are equivalent (`-db` == `--db`)
//! - `-name=value` and `-name value` are both accepted
//! - a boolean flag takes no value; `-read-auth` sets true, `-read-auth=false` unsets
//! - parsing stops at the first non-flag argument or a bare `--`
//! - an unknown flag is an error, never silently ignored

use std::collections::BTreeMap;

/// Result of parsing a flag set.
#[derive(Debug, Default, Clone)]
pub struct Parsed {
    values: BTreeMap<String, String>,
    pub rest: Vec<String>,
}

impl Parsed {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn string(&self, name: &str, default: &str) -> String {
        self.get(name).unwrap_or(default).to_string()
    }

    pub fn bool(&self, name: &str, default: bool) -> Result<bool, String> {
        match self.get(name) {
            None => Ok(default),
            Some("true") | Some("1") | Some("t") | Some("T") => Ok(true),
            Some("false") | Some("0") | Some("f") | Some("F") => Ok(false),
            Some(v) => Err(format!("invalid boolean value {v:?} for -{name}")),
        }
    }

    pub fn int(&self, name: &str, default: i64) -> Result<i64, String> {
        match self.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("invalid value {v:?} for flag -{name}: parse error")),
        }
    }

    pub fn float(&self, name: &str, default: f64) -> Result<f64, String> {
        match self.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("invalid value {v:?} for flag -{name}: parse error")),
        }
    }
}

/// Parse `args` (without the program name) against the names of the boolean
/// flags in `bools`; every other known name is string-valued by default.
pub fn parse(args: &[String], bools: &[&str]) -> Result<Parsed, String> {
    let mut out = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            out.rest.extend(args[i + 1..].iter().cloned());
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            out.rest.extend(args[i..].iter().cloned());
            break;
        }
        let body = arg.trim_start_matches('-');
        let (name, inline) = match body.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (body, None),
        };
        if name.is_empty() {
            return Err(format!("bad flag syntax: {arg}"));
        }
        let is_bool = bools.contains(&name);
        let value = match inline {
            Some(v) => v,
            None if is_bool => "true".to_string(),
            None => {
                i += 1;
                match args.get(i) {
                    Some(v) => v.clone(),
                    None => return Err(format!("flag needs an argument: -{name}")),
                }
            }
        };
        out.values.insert(name.to_string(), value);
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str], bools: &[&str]) -> Parsed {
        parse(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            bools,
        )
        .unwrap()
    }

    #[test]
    fn accepts_single_and_double_dash_and_equals() {
        let a = p(&["-db", "/tmp/x.db", "--listen=:9000"], &[]);
        assert_eq!(a.get("db"), Some("/tmp/x.db"));
        assert_eq!(a.get("listen"), Some(":9000"));
    }

    #[test]
    fn booleans_take_no_value() {
        let a = p(&["-read-auth", "-db", "x"], &["read-auth"]);
        assert!(a.bool("read-auth", false).unwrap());
        assert_eq!(a.get("db"), Some("x"));
    }

    #[test]
    fn stops_at_first_positional() {
        let a = p(&["add", "-db", "x"], &[]);
        assert_eq!(a.rest, vec!["add", "-db", "x"]);
    }
}
