//! Routing keys.
//!
//! A routing key is a plain string `<kind>:<body>` in local scope:
//!
//! | Kind | Example |
//! |---|---|
//! | path | `path:src/billing/invoice.rs` |
//! | symbol | `sym:rust:Invoice::total` |
//! | test | `test:tests::total_rounds` |
//! | module | `mod:src/billing` |
//!
//! The qualified name of a symbol is the chain of enclosing declaration names, joined with `::`.
//! A subscription may use `*` as the language (`sym:*:Money`).
//!
//! Team scope sends `HMAC(team_key, key)` to the hub. The hook for it is [`KeyMapper`]: the
//! transport maps each key before it leaves the daemon. [`PlainKeys`] is the local mapper.

/// The key of a file. `path` uses `/` separators (a `\` becomes `/`) and has no leading `./`.
pub fn path(path: &str) -> String {
    format!("path:{}", norm_path(path))
}

/// The key of a symbol. `language` is the lower case name of the language (for example `rust`).
pub fn sym(language: &str, qualified: &str) -> String {
    format!("sym:{language}:{qualified}")
}

/// The key of a test.
pub fn test(qualified: &str) -> String {
    format!("test:{qualified}")
}

/// The key of a folder.
pub fn module(folder: &str) -> String {
    let f = norm_path(folder);
    let f = f.trim_end_matches('/');
    format!("mod:{}", if f.is_empty() { "." } else { f })
}

/// The key of the folder that holds a file.
pub fn module_of(file: &str) -> String {
    let p = norm_path(file);
    match p.rsplit_once('/') {
        Some((folder, _)) => module(folder),
        None => module("."),
    }
}

/// The kind part of a key (`path`, `sym`, `test`, `mod`), or `None` for a bad key.
pub fn kind_of(key: &str) -> Option<&str> {
    let (kind, rest) = key.split_once(':')?;
    (matches!(kind, "path" | "sym" | "test" | "mod") && !rest.is_empty()).then_some(kind)
}

/// The language and the qualified name of a symbol key.
pub fn split_sym(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("sym:")?.split_once(':')
}

/// The last name of a qualified name (`Invoice::total` gives `total`).
pub fn last_name(qualified: &str) -> &str {
    qualified.rsplit("::").next().unwrap_or(qualified)
}

/// True when a subscription key matches an event key.
///
/// - Equal keys match.
/// - `sym:*:X` matches `sym:<any language>:X`.
/// - `mod:F` matches every path under the folder `F` (`mod:.` matches every path).
pub fn matches(subscription: &str, key: &str) -> bool {
    if subscription == key {
        return true;
    }
    if let (Some((sl, sq)), Some((kl, kq))) = (split_sym(subscription), split_sym(key)) {
        return sl == "*" && sq == kq && !kl.is_empty();
    }
    if let (Some(folder), Some(file)) =
        (subscription.strip_prefix("mod:"), key.strip_prefix("path:"))
    {
        return folder == "." || file.starts_with(&format!("{folder}/"));
    }
    false
}

/// Maps a local key to the form that leaves the daemon.
pub trait KeyMapper {
    /// The transport form of `key`.
    fn map(&self, key: &str) -> String;
}

/// The local mapper: the key stays as it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlainKeys;

impl KeyMapper for PlainKeys {
    fn map(&self, key: &str) -> String {
        key.to_owned()
    }
}

fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    s.strip_prefix("./").unwrap_or(&s).to_owned()
}
