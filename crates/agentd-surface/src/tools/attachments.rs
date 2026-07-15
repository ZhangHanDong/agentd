use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::error::SurfaceError;

pub(crate) const ATTACHMENT_MAX_ITEMS: usize = 8;
pub(crate) const ATTACHMENT_MAX_BYTES: u64 = 20 * 1024 * 1024;

pub(crate) fn normalize_local_attachments(raw: Vec<Value>) -> Result<Vec<Value>, SurfaceError> {
    normalize_attachments(raw, None)
}

pub(crate) fn normalize_http_attachments(
    raw: Vec<Value>,
    media_dir: &Path,
) -> Result<Vec<Value>, SurfaceError> {
    normalize_attachments(raw, Some(media_dir))
}

fn normalize_attachments(
    raw: Vec<Value>,
    media_dir: Option<&Path>,
) -> Result<Vec<Value>, SurfaceError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    if raw.len() > ATTACHMENT_MAX_ITEMS {
        return Err(SurfaceError::BadRequest(format!(
            "too many attachments (max {ATTACHMENT_MAX_ITEMS})"
        )));
    }
    raw.into_iter()
        .enumerate()
        .map(|(index, item)| {
            normalize_attachment(item, media_dir)
                .map_err(|err| SurfaceError::BadRequest(format!("attachments[{index}]: {err}")))
        })
        .collect()
}

fn normalize_attachment(raw: Value, media_dir: Option<&Path>) -> Result<Value, String> {
    let Value::Object(map) = raw else {
        return normalize_local_attachment(raw);
    };
    if map.get("staged").and_then(Value::as_bool).unwrap_or(false) {
        let Some(media_dir) = media_dir else {
            return normalize_local_attachment(Value::Object(map));
        };
        return normalize_staged_attachment(map, media_dir);
    }
    normalize_local_attachment(Value::Object(map))
}

fn normalize_local_attachment(raw: Value) -> Result<Value, String> {
    let (path, name, mime, kind) = match raw {
        Value::String(path) => {
            let path =
                clean_string(Some(path)).ok_or_else(|| "attachment.path required".to_string())?;
            let fallback = fallback_name(&path);
            let name = normalize_attachment_name(None, &fallback);
            let mime = None;
            let kind = infer_attachment_kind(None, mime.as_deref(), &name);
            (path, name, mime, kind)
        }
        Value::Object(mut map) => {
            let path = map
                .remove("path")
                .and_then(value_string)
                .and_then(|value| clean_string(Some(value)))
                .ok_or_else(|| "attachment.path required".to_string())?;
            if path.len() > 4096 {
                return Err("attachment.path too long".to_string());
            }
            let fallback = fallback_name(&path);
            let name =
                normalize_attachment_name(map.remove("name").and_then(value_string), &fallback);
            let mime = normalize_attachment_mime(map.remove("mime").and_then(value_string));
            let kind = infer_attachment_kind(
                map.remove("kind").and_then(value_string),
                mime.as_deref(),
                &name,
            );
            (path, name, mime, kind)
        }
        _ => return Err("attachment must be a string path or object".to_string()),
    };

    let size = readable_file_size(Path::new(&path))?;
    Ok(json!({
        "path": path,
        "name": name,
        "mime": mime,
        "kind": kind,
        "size": size,
        "staged": false,
        "source_path": path,
    }))
}

fn normalize_staged_attachment(
    mut map: serde_json::Map<String, Value>,
    media_dir: &Path,
) -> Result<Value, String> {
    let path = map
        .remove("path")
        .and_then(value_string)
        .and_then(|value| clean_string(Some(value)))
        .ok_or_else(|| "attachment.path required".to_string())?;
    if path.len() > 4096 {
        return Err("attachment.path too long".to_string());
    }
    let resolved = resolve_media_path(&path, media_dir)?;
    let size = readable_file_size(&resolved)?;
    let fallback = fallback_name(&path);
    let name = normalize_attachment_name(map.remove("name").and_then(value_string), &fallback);
    let mime = normalize_attachment_mime(map.remove("mime").and_then(value_string));
    let kind = infer_attachment_kind(
        map.remove("kind").and_then(value_string),
        mime.as_deref(),
        &name,
    );
    let source_path = map
        .remove("source_path")
        .and_then(value_string)
        .and_then(|value| clean_string(Some(value)));

    Ok(json!({
        "path": resolved.to_string_lossy().to_string(),
        "name": name,
        "mime": mime,
        "kind": kind,
        "size": size,
        "staged": true,
        "source_path": source_path,
    }))
}

fn readable_file_size(path: &Path) -> Result<u64, String> {
    let stat = fs::metadata(path)
        .map_err(|e| format!("attachment not found: {} ({e})", path.display()))?;
    if !stat.is_file() {
        return Err(format!("attachment is not a file: {}", path.display()));
    }
    let size = stat.len();
    if size == 0 {
        return Err(format!("attachment is empty: {}", path.display()));
    }
    if size > ATTACHMENT_MAX_BYTES {
        return Err(format!(
            "attachment too large: {} ({size} bytes > {ATTACHMENT_MAX_BYTES})",
            path.display()
        ));
    }
    Ok(size)
}

fn value_string(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn clean_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn fallback_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("file")
        .to_string()
}

pub(crate) fn normalize_attachment_name(value: Option<String>, fallback: &str) -> String {
    let raw = clean_string(value).unwrap_or_else(|| fallback.to_string());
    let base = Path::new(&raw)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(fallback);
    let mut out = String::new();
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | '(' | ')' | '[' | ']' | ' ')
        {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    clean_string(Some(out)).unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn normalize_attachment_mime(value: Option<String>) -> Option<String> {
    let value = clean_string(value)?.to_ascii_lowercase();
    let (left, right) = value.split_once('/')?;
    if left.is_empty() || right.is_empty() {
        return None;
    }
    let valid = |part: &str| {
        part.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '!' | '#' | '$' | '&' | '^' | '_' | '.' | '+' | '-')
        })
    };
    (valid(left) && valid(right)).then_some(value)
}

pub(crate) fn infer_attachment_kind(
    raw_kind: Option<String>,
    mime: Option<&str>,
    name: &str,
) -> String {
    if let Some(kind) = raw_kind
        && (kind == "image" || kind == "file")
    {
        return kind;
    }
    if mime.is_some_and(|mime| mime.starts_with("image/")) {
        return "image".to_string();
    }
    let lower = name.to_ascii_lowercase();
    if [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg", ".avif", ".heic", ".heif",
        ".tif", ".tiff",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
    {
        return "image".to_string();
    }
    "file".to_string()
}

pub(crate) fn resolve_media_path(raw_path: &str, media_dir: &Path) -> Result<PathBuf, String> {
    let requested =
        clean_string(Some(raw_path.to_string())).ok_or_else(|| "path required".to_string())?;
    if requested.len() > 4096 {
        return Err("path too long".to_string());
    }
    let root = normalize_absolute(media_dir)?;
    let path = normalize_absolute(Path::new(&requested))?;
    if path == root || path.starts_with(&root) {
        Ok(path)
    } else {
        Err("path not allowed".to_string())
    }
}

pub(crate) fn normalize_absolute(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("current dir unavailable: {e}"))?
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(value) => out.push(value),
        }
    }
    Ok(out)
}
