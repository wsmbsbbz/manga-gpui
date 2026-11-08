//! Decodes the image list hidden in a chapter page.
//!
//! The page carries a Dean Edwards style "packed" script:
//! `eval(function(p,a,c,k,e,d){…}('<template>',62,96,'<lz>'['splic']('|'),0,{}))`.
//! `<template>` is JS source with every identifier replaced by a base-`a`
//! token, and `<lz>` is an LZString-compressed `|`-separated keyword table
//! (`splic` is the site's own LZString-then-split helper). Substituting the
//! keywords back yields `SMH.imgData({...json...}).preInit();`.
//!
//! This is done as plain data decoding — no JavaScript is executed.

use serde::Deserialize;

use crate::lzstring;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ImgData {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub sl: Sl,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Sl {
    #[serde(default, deserialize_with = "string_or_number")]
    pub e: String,
    #[serde(default, deserialize_with = "string_or_number")]
    pub m: String,
}

fn string_or_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Null => String::new(),
        v => v.to_string(),
    })
}

/// Finds the packed script in a chapter page's HTML and decodes it.
pub fn img_data_from_html(html: &str) -> Result<ImgData, String> {
    let at = html.find("p,a,c,k,e,d").ok_or("页面中没有图片数据脚本")?;
    let script = &html[at..];
    let end = script.find("</script>").unwrap_or(script.len());
    img_data_from_script(&script[..end])
}

pub fn img_data_from_script(script: &str) -> Result<ImgData, String> {
    let unpacked = unpack(script)?;
    let at = unpacked.find("imgData(").ok_or("解包结果中没有 imgData")?;
    let rest = &unpacked[at + "imgData(".len()..];
    let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<ImgData>();
    match stream.next() {
        Some(Ok(data)) if !data.files.is_empty() => Ok(data),
        Some(Ok(_)) => Err("章节没有图片".into()),
        Some(Err(e)) => Err(format!("imgData 解析失败：{e}")),
        None => Err("imgData 为空".into()),
    }
}

/// Reverses the packer: returns the original JS source.
pub fn unpack(script: &str) -> Result<String, String> {
    // Arguments start right after the packer function body: `}('`.
    let at = script.rfind("}('").ok_or("找不到打包参数")?;
    let mut cur = Cursor { s: &script[at + 2..] };
    let template = cur.js_string()?;
    cur.expect(',')?;
    let radix = cur.int()?;
    cur.expect(',')?;
    let count = cur.int()?;
    cur.expect(',')?;
    let packed_keywords = cur.js_string()?;
    if !(2..=62).contains(&radix) {
        return Err(format!("不支持的进制 {radix}"));
    }
    let table = lzstring::decompress_from_base64(&packed_keywords).ok_or("关键字表解压失败")?;
    let keywords: Vec<&str> = table.split('|').collect();

    let mut dict = std::collections::HashMap::with_capacity(count as usize);
    for c in 0..count {
        let token = encode_token(c, radix);
        let word = keywords.get(c as usize).copied().filter(|w| !w.is_empty()).unwrap_or(&token).to_string();
        dict.insert(token, word);
    }
    Ok(replace_words(&template, |w| dict.get(w).map(String::as_str)))
}

/// The packer's `e(c)`: base-`radix` digits `0-9a-zA-Z`.
fn encode_token(c: u32, radix: u32) -> String {
    let digit =
        |d: u32| -> char { if d > 35 { char::from_u32(d + 29).unwrap() } else { char::from_digit(d, 36).unwrap() } };
    let mut out = String::new();
    if c >= radix {
        out.push_str(&encode_token(c / radix, radix));
    }
    out.push(digit(c % radix));
    out
}

/// Replaces every `\w+` run (ASCII word chars, like JS `\b\w+\b`).
fn replace_words<'a>(s: &str, lookup: impl Fn(&str) -> Option<&'a str>) -> String {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(s.len() * 2);
    let mut start = None;
    for (i, ch) in s.char_indices() {
        match (is_word(ch), start) {
            (true, None) => start = Some(i),
            (false, Some(st)) => {
                let w = &s[st..i];
                out.push_str(lookup(w).unwrap_or(w));
                out.push(ch);
                start = None;
            }
            (false, None) => out.push(ch),
            (true, Some(_)) => {}
        }
    }
    if let Some(st) = start {
        let w = &s[st..];
        out.push_str(lookup(w).unwrap_or(w));
    }
    out
}

struct Cursor<'a> {
    s: &'a str,
}

impl Cursor<'_> {
    fn skip_ws(&mut self) {
        self.s = self.s.trim_start();
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        self.skip_ws();
        self.s = self.s.strip_prefix(c).ok_or_else(|| format!("期望 `{c}`"))?;
        Ok(())
    }

    fn int(&mut self) -> Result<u32, String> {
        self.skip_ws();
        let end = self.s.find(|c: char| !c.is_ascii_digit()).unwrap_or(self.s.len());
        let n = self.s[..end].parse().map_err(|_| "期望整数".to_string())?;
        self.s = &self.s[end..];
        Ok(n)
    }

    /// A single- or double-quoted JS string literal with escapes.
    fn js_string(&mut self) -> Result<String, String> {
        self.skip_ws();
        let mut chars = self.s.char_indices();
        let quote = match chars.next() {
            Some((_, q @ ('\'' | '"'))) => q,
            _ => return Err("期望字符串".into()),
        };
        let mut out = String::new();
        while let Some((i, ch)) = chars.next() {
            if ch == quote {
                self.s = &self.s[i + 1..];
                return Ok(out);
            }
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            let (_, esc) = chars.next().ok_or("字符串未结束")?;
            match esc {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '0' => out.push('\0'),
                'x' | 'u' => {
                    let len = if esc == 'x' { 2 } else { 4 };
                    let hex: String = (0..len).filter_map(|_| chars.next().map(|(_, c)| c)).collect();
                    let code = u32::from_str_radix(&hex, 16).map_err(|_| "错误的转义")?;
                    out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                }
                other => out.push(other),
            }
        }
        Err("字符串未结束".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = include_str!("../testdata/chapter_script.js");

    #[test]
    fn token_encoding_matches_packer() {
        assert_eq!(encode_token(0, 62), "0");
        assert_eq!(encode_token(10, 62), "a");
        assert_eq!(encode_token(35, 62), "z");
        assert_eq!(encode_token(36, 62), "A");
        assert_eq!(encode_token(61, 62), "Z");
        assert_eq!(encode_token(62, 62), "10");
        assert_eq!(encode_token(125, 62), "21");
    }

    #[test]
    fn replace_words_only_touches_whole_words() {
        let out = replace_words("a.b(c)+ab", |w| match w {
            "a" => Some("X"),
            "b" => Some("Y"),
            _ => None,
        });
        assert_eq!(out, "X.Y(c)+ab");
    }

    #[test]
    fn js_string_handles_escapes() {
        let mut c = Cursor { s: r#"'it\'s \x7c \\ ok',1"# };
        assert_eq!(c.js_string().unwrap(), r"it's | \ ok");
        assert_eq!(c.s, ",1");
    }

    #[test]
    fn decodes_real_chapter_script() {
        let unpacked = unpack(SCRIPT).unwrap();
        assert!(unpacked.contains("SMH.imgData("), "{unpacked}");
        let data = img_data_from_script(SCRIPT).unwrap();
        assert_eq!(data.files.len(), 12, "{data:?}");
        assert!(data.path.starts_with('/'), "{}", data.path);
        assert!(data.path.ends_with('/'), "{}", data.path);
        assert!(!data.sl.e.is_empty() && !data.sl.m.is_empty(), "{:?}", data.sl);
        assert!(data.files.iter().all(|f| f.contains('.')), "{:?}", data.files);
    }

    #[test]
    fn finds_script_in_html() {
        let html = format!("<html><script>var x=1;</script><script>{SCRIPT}</script></html>");
        assert_eq!(img_data_from_html(&html).unwrap(), img_data_from_script(SCRIPT).unwrap());
        assert!(img_data_from_html("<html></html>").is_err());
    }
}
