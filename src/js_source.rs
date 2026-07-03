use crate::config::AnalyzeConfig;
use crate::fs::normalize_path;
use crate::source::{LineMarker, SourceUnit};
use anyhow::{Context, Result};
use globset::{Glob, GlobSetBuilder};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaScriptSyntax {
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
}

const JS_DEFAULT_EXCLUDES: &[&str] = &[
    "**/.git/**",
    "**/node_modules/**",
    "**/.pnpm-store/**",
    "**/.cache/**",
    "**/.turbo/**",
    "**/dist/**",
    "**/build/**",
    "**/coverage/**",
];

pub fn discover_js_sources(config: &AnalyzeConfig) -> Result<Vec<SourceUnit>> {
    let root = config
        .input
        .canonicalize()
        .with_context(|| format!("input path does not exist: {}", config.input.display()))?;
    let excludes = build_excludes(config)?;
    let mut units = Vec::new();

    for entry in WalkDir::new(&root) {
        let entry = entry.map_err(|err| {
            let path = err.path().unwrap_or(root.as_path()).display().to_string();
            anyhow::Error::new(err).context(format!("failed to traverse {path}"))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let relative_path = normalize_path(rel);
        if is_excluded(&excludes, &relative_path) || !is_js_family_path(path) {
            continue;
        }

        let source_text = fs::read_to_string(path)
            .with_context(|| format!("failed to read source file {}", path.display()))?;
        if path.extension().and_then(|s| s.to_str()) == Some("vue") {
            units.extend(extract_vue_script_units(
                path,
                &relative_path,
                &source_text,
            )?);
        } else {
            units.push(SourceUnit {
                absolute_path: path.to_path_buf(),
                relative_path,
                source_text,
                original_path: None,
                line_markers: Vec::new(),
            });
        }
    }

    units.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(units)
}

pub fn extract_vue_script_units(
    absolute_path: &Path,
    relative_path: &str,
    source_text: &str,
) -> Result<Vec<SourceUnit>> {
    let mut units = Vec::new();
    let mut cursor = 0;

    while let Some(script_start) = find_next_script_start(source_text, cursor) {
        let Some(open_end_offset) = source_text[script_start..].find('>') else {
            break;
        };
        let open_end = script_start + open_end_offset;
        let attrs = &source_text[script_start + "<script".len()..open_end];
        let raw_content_start = open_end + 1;
        let content_start = first_content_line_start(source_text, raw_content_start);
        let Some(close_offset) = source_text[content_start..].find("</script>") else {
            break;
        };
        let content_end = content_start + close_offset;
        let source = source_text[content_start..content_end].to_string();
        let script_kind = if has_attr(attrs, "setup") {
            "setup"
        } else {
            "normal"
        };
        let lang = script_lang(attrs);

        units.push(SourceUnit {
            absolute_path: absolute_path.to_path_buf(),
            relative_path: format!("{relative_path}?script={script_kind}&lang={lang}"),
            source_text: source,
            original_path: Some(absolute_path.to_path_buf()),
            line_markers: vec![LineMarker {
                generated_line: 1,
                original_file: relative_path.to_string(),
                original_line: line_number_at(source_text, content_start),
            }],
        });

        cursor = content_end + "</script>".len();
    }

    Ok(units)
}

pub fn syntax_for_unit(unit: &SourceUnit) -> JavaScriptSyntax {
    if unit.relative_path.ends_with(".tsx") {
        JavaScriptSyntax::Tsx
    } else if unit.relative_path.ends_with(".ts") || unit.relative_path.contains("&lang=ts") {
        JavaScriptSyntax::TypeScript
    } else if unit.relative_path.ends_with(".jsx") {
        JavaScriptSyntax::Jsx
    } else {
        JavaScriptSyntax::JavaScript
    }
}

fn build_excludes(config: &AnalyzeConfig) -> Result<globset::GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in config
        .exclude
        .iter()
        .map(String::as_str)
        .chain(JS_DEFAULT_EXCLUDES.iter().copied())
    {
        builder.add(Glob::new(pattern).with_context(|| format!("invalid exclude glob {pattern}"))?);
    }
    Ok(builder.build()?)
}

fn is_excluded(excludes: &globset::GlobSet, relative_path: &str) -> bool {
    excludes.is_match(relative_path) || excludes.is_match(format!("/{relative_path}"))
}

fn is_js_family_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "vue")
    )
}

fn find_next_script_start(source_text: &str, mut cursor: usize) -> Option<usize> {
    loop {
        let rest = &source_text[cursor..];
        let script_offset = rest.find("<script")?;
        let comment_offset = rest.find("<!--");

        if let Some(comment_offset) = comment_offset
            && comment_offset < script_offset
        {
            let comment_start = cursor + comment_offset;
            let Some(comment_end_offset) = source_text[comment_start + "<!--".len()..].find("-->")
            else {
                return None;
            };
            cursor = comment_start + "<!--".len() + comment_end_offset + "-->".len();
            continue;
        }

        return Some(cursor + script_offset);
    }
}

fn has_attr(attrs: &str, attr: &str) -> bool {
    iter_attrs(attrs).any(|(name, _)| name.eq_ignore_ascii_case(attr))
}

fn script_lang(attrs: &str) -> &'static str {
    if iter_attrs(attrs).any(|(name, value)| {
        name.eq_ignore_ascii_case("lang")
            && value.is_some_and(|value| value.eq_ignore_ascii_case("ts"))
    }) {
        "ts"
    } else {
        "js"
    }
}

fn iter_attrs(attrs: &str) -> impl Iterator<Item = (&str, Option<&str>)> {
    AttrIter { attrs, cursor: 0 }
}

struct AttrIter<'a> {
    attrs: &'a str,
    cursor: usize,
}

impl<'a> Iterator for AttrIter<'a> {
    type Item = (&'a str, Option<&'a str>);

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.attrs.as_bytes();
        while self.cursor < bytes.len()
            && (bytes[self.cursor].is_ascii_whitespace() || bytes[self.cursor] == b'/')
        {
            self.cursor += 1;
        }
        if self.cursor >= bytes.len() {
            return None;
        }

        let name_start = self.cursor;
        while self.cursor < bytes.len()
            && !bytes[self.cursor].is_ascii_whitespace()
            && bytes[self.cursor] != b'='
            && bytes[self.cursor] != b'/'
        {
            self.cursor += 1;
        }
        let name = &self.attrs[name_start..self.cursor];

        while self.cursor < bytes.len() && bytes[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
        if self.cursor >= bytes.len() || bytes[self.cursor] != b'=' {
            return Some((name, None));
        }

        self.cursor += 1;
        while self.cursor < bytes.len() && bytes[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
        if self.cursor >= bytes.len() {
            return Some((name, Some("")));
        }

        let quote = bytes[self.cursor];
        let value = if quote == b'\'' || quote == b'"' {
            self.cursor += 1;
            let value_start = self.cursor;
            while self.cursor < bytes.len() && bytes[self.cursor] != quote {
                self.cursor += 1;
            }
            let value = &self.attrs[value_start..self.cursor];
            if self.cursor < bytes.len() {
                self.cursor += 1;
            }
            value
        } else {
            let value_start = self.cursor;
            while self.cursor < bytes.len()
                && !bytes[self.cursor].is_ascii_whitespace()
                && bytes[self.cursor] != b'/'
            {
                self.cursor += 1;
            }
            &self.attrs[value_start..self.cursor]
        };

        Some((name, Some(value)))
    }
}

fn line_number_at(source_text: &str, byte_offset: usize) -> usize {
    source_text[..byte_offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn first_content_line_start(source_text: &str, content_start: usize) -> usize {
    if source_text[content_start..].starts_with("\r\n") {
        content_start + 2
    } else if source_text[content_start..].starts_with('\n')
        || source_text[content_start..].starts_with('\r')
    {
        content_start + 1
    } else {
        content_start
    }
}
