//! Small forms for the screens: the checks of what the user types (before a call to the daemon
//! that would refuse it), the folder picker, and the widgets that the forms share.

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, ElementId, FontWeight, PathPromptOptions, SharedString, Stateful,
    div, px, rgb, rgba,
};

use crate::theme::*;

/// Characters that a folder name cannot hold on Windows (and `/` everywhere).
const FORBIDDEN_IN_NAME: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// The folder name of a new project.
pub fn project_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("Enter a name for the project.".into());
    }
    if name.chars().count() > 80 {
        return Err("The name has more than 80 characters.".into());
    }
    if name == "." || name == ".." || name.ends_with('.') {
        return Err("The name cannot be a dot or end with a dot.".into());
    }
    if name
        .chars()
        .any(|c| FORBIDDEN_IN_NAME.contains(&c) || c.is_control())
    {
        return Err("The name cannot contain / \\ : * ? \" < > |.".into());
    }
    Ok(name.to_string())
}

/// The name of a part of a codebase map.
pub fn part_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("Enter a name for the part.".into());
    }
    if name.chars().count() > 40 {
        return Err("The name has more than 40 characters.".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("Use letters, digits, - _ and . in the name.".into());
    }
    Ok(name.to_string())
}

/// The path patterns of a part: split at commas, spaces and line breaks. A pattern is relative
/// to the root of the repository and uses `/`.
pub fn globs(raw: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if list.is_empty() {
        return Err("Enter at least one path pattern, for example crates/pay/**.".into());
    }
    for glob in &list {
        if glob.starts_with('/') || glob.contains(":\\") || glob.contains(":/") {
            return Err(format!("'{glob}' is not relative to the repository."));
        }
        if glob.contains('\\') {
            return Err(format!("'{glob}' uses a backslash. Use / in a pattern."));
        }
        if glob.split('/').any(|part| part == "..") {
            return Err(format!("'{glob}' leaves the repository."));
        }
    }
    Ok(list)
}

/// The name of an ACP agent.
pub fn acp_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("Enter a name for the agent.".into());
    }
    if name.chars().count() > 40 {
        return Err("The name has more than 40 characters.".into());
    }
    if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err("The name needs a letter or a digit.".into());
    }
    if name.chars().any(char::is_control) {
        return Err("The name cannot hold a control character.".into());
    }
    Ok(name.to_string())
}

/// The program of an ACP agent: a name on the PATH, or the path of a file.
pub fn acp_command(raw: &str) -> Result<String, String> {
    let command = raw.trim();
    if command.is_empty() {
        return Err("Enter the command that starts the agent.".into());
    }
    if command.chars().any(char::is_control) {
        return Err("The command cannot hold a line break.".into());
    }
    Ok(command.to_string())
}

/// The arguments of a command line: split at spaces; a quoted part (`"a b"` or `'a b'`) stays
/// one argument.
pub fn split_args(raw: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in raw.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return Err("Close the quote in the arguments.".into());
    }
    if started || !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

/// Pick one folder. `done` runs with the folder; a closed dialog does nothing.
pub fn pick_folder<T: 'static>(
    cx: &mut Context<T>,
    done: impl FnOnce(&mut T, PathBuf, &mut Context<T>) + 'static,
) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    cx.spawn(async move |this, cx| {
        if let Ok(Ok(Some(paths))) = picked.await
            && let Some(path) = paths.into_iter().next()
        {
            let _ = this.update(cx, |this, cx| done(this, path, cx));
        }
    })
    .detach();
}

// ---- widgets --------------------------------------------------------------------------------

/// A label over a control.
pub fn field(label: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(div().text_color(rgb(tk().ink_2)).child(label))
        .child(control)
}

/// The box around a text field.
pub fn boxed() -> Div {
    div()
        .h(px(36.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(tk().r_button)
        .border_1()
        .border_color(rgb(tk().field_line))
        .text_size(crate::ui::ts(crate::theme::T_BODY))
        .when(crate::ui::irix(), |d| {
            d.rounded(px(0.))
                .border_0()
                .bg(rgb(0xFFFFFF))
                .text_color(rgb(0x111111))
        })
        .when(crate::ui::irix(), |d| crate::ui::bevel_edges(false, d))
}

/// A button of a form. `busy` dims it.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    busy: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(14.))
        .flex()
        .flex_none()
        .items_center()
        .rounded(tk().r_button)
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .when(busy, |d| d.opacity(0.6))
        .when(primary && !crate::ui::irix(), |d| {
            d.bg(rgb(tk().primary))
                .text_color(rgb(tk().card))
                .hover(|s| s.bg(rgb(tk().primary_hover)))
        })
        .when(!primary && !crate::ui::irix(), |d| {
            d.border_1()
                .border_color(rgb(tk().edge))
                .hover(|s| s.border_color(rgb(tk().muted)))
        })
        .when(crate::ui::irix(), |d| {
            d.rounded(px(0.))
                .border_0()
                .bg(rgb(0xC8C8C8))
                .text_color(rgb(0x111111))
                .hover(|s| s.bg(rgb(0xD8D8D8)))
                .when(primary, |d| d.border_2().border_color(rgb(0x111111)))
        })
        .when(crate::ui::irix(), |d| crate::ui::bevel_edges(true, d))
        .child(label.into())
}

/// The line of an error under a form.
pub fn error_line(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .text_color(rgb(tk().e_text))
        .child(text.into())
}

/// A choice chip (a template, a kind).
pub fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(if on {
            rgb(tk().primary)
        } else {
            rgba(tk().line_2)
        })
        .text_size(crate::ui::ts(crate::theme::T_SMALL))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(tk().muted)))
        .when(crate::ui::irix(), |d| {
            d.rounded(px(0.))
                .border_0()
                .bg(rgb(if on { 0xA8A8A8 } else { 0xC8C8C8 }))
                .text_color(rgb(0x111111))
        })
        .when(crate::ui::irix(), |d| crate::ui::bevel_edges(!on, d))
        .child(label.into())
}

/// A path that does not fit: its tail with a leading `…`.
pub fn tail(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max {
        return path.to_string();
    }
    let skip = count - max + 1;
    format!("…{}", path.chars().skip(skip).collect::<String>())
}

/// The element of a path that the user picked, or a hint.
pub fn path_box(path: Option<&str>, hint: &'static str) -> AnyElement {
    let (text, color) = match path {
        Some(p) => (tail(p, 44), tk().ink),
        None => (hint.to_string(), tk().muted),
    };
    div()
        .min_w(px(0.))
        .flex_1()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .text_color(rgb(color))
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_name_is_a_plain_folder_name() {
        assert_eq!(project_name("  shop-api "), Ok("shop-api".to_string()));
        assert!(project_name("").is_err());
        assert!(project_name("   ").is_err());
        assert!(project_name("a/b").is_err());
        assert!(project_name("a\\b").is_err());
        assert!(project_name("..").is_err());
        assert!(project_name("what?").is_err());
        assert!(project_name("dot.").is_err());
        assert!(project_name(&"x".repeat(81)).is_err());
        assert!(project_name("My Project 2").is_ok());
    }

    #[test]
    fn a_part_name_is_a_key() {
        assert_eq!(part_name("payment"), Ok("payment".to_string()));
        assert!(part_name("pay ment").is_err());
        assert!(part_name("").is_err());
        assert!(part_name("ü").is_err());
        assert!(part_name("a.b-c_d").is_ok());
    }

    #[test]
    fn globs_split_at_commas_and_spaces_and_stay_in_the_repository() {
        assert_eq!(
            globs("crates/pay/**, docs/*.md\nsrc/a.rs"),
            Ok(vec![
                "crates/pay/**".to_string(),
                "docs/*.md".to_string(),
                "src/a.rs".to_string()
            ])
        );
        assert!(globs("").is_err());
        assert!(globs(" , ").is_err());
        assert!(globs("/etc/**").is_err());
        assert!(globs("C:/work/**").is_err());
        assert!(globs("src\\a").is_err());
        assert!(globs("../x").is_err());
        assert!(globs("a/../b").is_err());
        assert!(globs("a..b").is_ok());
    }

    #[test]
    fn an_acp_agent_needs_a_name_and_a_command() {
        assert_eq!(acp_name(" Goose 2 "), Ok("Goose 2".to_string()));
        assert!(acp_name("").is_err());
        assert!(acp_name("---").is_err());
        assert!(acp_name(&"n".repeat(41)).is_err());
        assert_eq!(acp_command(" goose "), Ok("goose".to_string()));
        assert!(acp_command("  ").is_err());
        assert!(acp_command("a\nb").is_err());
    }

    #[test]
    fn arguments_split_like_a_command_line() {
        assert_eq!(split_args(""), Ok(vec![]));
        assert_eq!(split_args("  "), Ok(vec![]));
        assert_eq!(
            split_args("acp --model \"big one\" 'x y'"),
            Ok(vec![
                "acp".to_string(),
                "--model".to_string(),
                "big one".to_string(),
                "x y".to_string()
            ])
        );
        assert_eq!(
            split_args("a \"\" b"),
            Ok(vec!["a".into(), "".into(), "b".into()])
        );
        assert!(split_args("a \"b").is_err());
    }

    #[test]
    fn a_long_path_keeps_its_tail() {
        assert_eq!(tail("C:/a/b", 20), "C:/a/b");
        let long = tail("C:/work/projects/very/deep/folder", 12);
        assert!(long.starts_with('…') && long.ends_with("folder"));
        assert_eq!(long.chars().count(), 12);
    }
}
