//! The front matter of a page: a small, strict subset of YAML (`key: value` lines).

use crate::model::Status;

#[derive(Debug, PartialEq, Eq)]
pub struct FrontMatter {
    pub title: String,
    pub description: String,
    pub status: Status,
    pub order: i64,
}

/// A problem in the front matter, with the 1-based line of the file it is on.
#[derive(Debug, PartialEq, Eq)]
pub struct FrontMatterError {
    pub line: usize,
    pub message: String,
}

fn fail<T>(line: usize, message: impl Into<String>) -> Result<T, FrontMatterError> {
    Err(FrontMatterError {
        line,
        message: message.into(),
    })
}

/// Splits a page into its front matter, its Markdown body and the line the body starts on.
pub fn split(text: &str) -> Result<(FrontMatter, &str, usize), FrontMatterError> {
    let mut lines = text.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return fail(1, "the page must start with a `---` front matter block"),
    }
    let mut consumed = text.find('\n').map_or(text.len(), |at| at + 1);
    let mut fields: Vec<(usize, &str, String)> = Vec::new();
    let mut closed_at = None;
    for (index, line) in lines.enumerate() {
        let number = index + 2;
        consumed += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        if content == "---" {
            closed_at = Some(number);
            break;
        }
        if content.trim().is_empty() || content.starts_with('#') {
            continue;
        }
        let Some((key, value)) = content.split_once(':') else {
            return fail(number, "expected `key: value`");
        };
        if key.is_empty() || key.starts_with(char::is_whitespace) {
            return fail(number, "nested or indented values are not supported");
        }
        fields.push((number, key.trim(), unquote(value.trim(), number)?));
    }
    let Some(closed_at) = closed_at else {
        return fail(1, "the front matter block is never closed with `---`");
    };
    let front = build(&fields, closed_at)?;
    Ok((front, &text[consumed..], closed_at + 1))
}

fn unquote(value: &str, line: usize) -> Result<String, FrontMatterError> {
    for quote in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(quote) {
            return match rest.strip_suffix(quote) {
                Some(inner) => Ok(inner.to_owned()),
                None => fail(
                    line,
                    format!("the value starts with {quote} but does not end with it"),
                ),
            };
        }
    }
    Ok(value.to_owned())
}

fn build(
    fields: &[(usize, &str, String)],
    end_line: usize,
) -> Result<FrontMatter, FrontMatterError> {
    let mut title = None;
    let mut description = None;
    let mut status = None;
    let mut order = None;
    for (line, key, value) in fields {
        let slot = match *key {
            "title" => &mut title,
            "description" => &mut description,
            "status" => &mut status,
            "order" => &mut order,
            other => return fail(*line, format!("unknown front matter field `{other}`")),
        };
        if slot.replace((*line, value.clone())).is_some() {
            return fail(*line, format!("`{key}` is set twice"));
        }
    }
    let require = |slot: Option<(usize, String)>, name: &str| match slot {
        Some((line, value)) if !value.is_empty() => Ok((line, value)),
        Some((line, _)) => fail(line, format!("`{name}` is empty")),
        None => fail(end_line, format!("missing `{name}`")),
    };
    let (_, title) = require(title, "title")?;
    let (_, description) = require(description, "description")?;
    let (status_line, status) = require(status, "status")?;
    let (order_line, order) = require(order, "order")?;
    let Some(status) = Status::parse(&status) else {
        return fail(
            status_line,
            format!("`status` is `{status}`; use implemented, partial or planned"),
        );
    };
    let Ok(order) = order.parse::<i64>() else {
        return fail(order_line, format!("`order` is `{order}`; use an integer"));
    };
    Ok(FrontMatter {
        title,
        description,
        status,
        order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "---\ntitle: Branches\ndescription: \"Work with: branches.\"\nstatus: partial\norder: 40\n---\n\nBody.\n";

    #[test]
    fn reads_the_fields_and_finds_the_body() {
        let (front, body, line) = split(PAGE).unwrap();
        assert_eq!(front.title, "Branches");
        assert_eq!(front.description, "Work with: branches.");
        assert_eq!(front.status, Status::Partial);
        assert_eq!(front.order, 40);
        assert_eq!(body, "\nBody.\n");
        assert_eq!(line, 7);
    }

    #[test]
    fn rejects_a_page_without_front_matter() {
        let error = split("# Title\n").unwrap_err();
        assert_eq!(error.line, 1);
    }

    #[test]
    fn rejects_unknown_missing_and_bad_values() {
        let unknown = PAGE.replace("order: 40", "order: 40\nweight: 3");
        assert!(split(&unknown).unwrap_err().message.contains("weight"));
        let missing = PAGE.replace("order: 40\n", "");
        assert!(
            split(&missing)
                .unwrap_err()
                .message
                .contains("missing `order`")
        );
        let status = PAGE.replace("partial", "done");
        assert!(
            split(&status)
                .unwrap_err()
                .message
                .contains("implemented, partial or planned")
        );
        let order = PAGE.replace("40", "forty");
        assert!(split(&order).unwrap_err().message.contains("integer"));
    }

    #[test]
    fn rejects_an_unclosed_block_and_nested_values() {
        assert!(
            split("---\ntitle: A\n")
                .unwrap_err()
                .message
                .contains("never closed")
        );
        assert!(
            split("---\n  title: A\n---\n")
                .unwrap_err()
                .message
                .contains("indented")
        );
    }
}
