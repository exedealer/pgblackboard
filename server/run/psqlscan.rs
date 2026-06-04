// TODO vibed, need rewrite

/// Returns the number of bytes consumed by the first SQL statement in `sql`,
/// including the terminating `;` and any trailing whitespace/comments after it.
/// If there is no `;` the full input length is returned (unterminated statement).
pub fn statement_boundary(sql: &[u8]) -> usize {
  // https://github.com/postgres/postgres/blob/REL_18_4/src/fe_utils/psqlscan.l
  let mut s = sql;
  let mut paren_depth: i32 = 0;
  let mut begin_depth: i32 = 0;
  let mut identifier_count: usize = 0;
  let mut identifiers = [0u8; 4];

  while !s.is_empty() {
    match s {
      // ── line comment ─────────────────────────────────────────────────
      [b'-', b'-', tail @ ..] => {
        s = tail;
        while let [b, tail @ ..] = s {
          s = tail;
          if matches!(b, b'\n' | b'\r') { break; }
        }
      }
      // ── block comment (nested) ───────────────────────────────────────
      [b'/', b'*', tail @ ..] => {
        s = tail;
        scan_block_comment(&mut s);
      }
      // ── dollar-quoted string: $tag$...$tag$ ──────────────────────────
      [b'$', ..] => {
        if let Some(tag_len) = dollar_tag_len(s) {
          let tag = &s[..tag_len];
          s = &s[tag_len..];
          while !s.is_empty() {
            if s.starts_with(tag) { s = &s[tag_len..]; break; }
            s = &s[1..];
          }
        } else {
          s = &s[1..]; // bare $ (e.g. $1 parameter)
        }
      }
      // ── B'...' X'...' — no escape processing, scan to first ' ───────
      [b'b' | b'B' | b'x' | b'X', b'\'', tail @ ..] => {
        s = tail;
        while let [b, tail @ ..] = s {
          s = tail;
          if *b == b'\'' { break; }
        }
      }
      // ── U&"..." unicode identifier ───────────────────────────────────
      [b'u' | b'U', b'&', b'"', tail @ ..] => { s = tail; scan_dquoted(&mut s); }
      // ── U&'...' unicode string — '' escape only, no backslash ────────
      [b'u' | b'U', b'&', b'\'', tail @ ..] => { s = tail; scan_squoted(&mut s, false); }
      // ── E'...' N'...' — backslash escapes ───────────────────────────
      [b'e' | b'E', b'\'', tail @ ..] => { s = tail; scan_squoted(&mut s, true); }
      // ── '...' ────────────────────────────────────────────────────────
      [b'\'', tail @ ..] => { s = tail; scan_squoted(&mut s, false); }
      // ── "..." double-quoted identifier ───────────────────────────────
      [b'"', tail @ ..] => { s = tail; scan_dquoted(&mut s); }
      // ── parentheses ──────────────────────────────────────────────────
      [b'(', tail @ ..] => { paren_depth += 1; s = tail; }
      [b')', tail @ ..] => { if paren_depth > 0 { paren_depth -= 1; } s = tail; }
      // ── semicolon ────────────────────────────────────────────────────
      [b';', tail @ ..] => {
        s = tail;
        if paren_depth == 0 && begin_depth == 0 {
          skip_ws_comments(&mut s);
          return sql.len() - s.len();
        }
      }
      // ── identifier — tracks BEGIN/END inside CREATE FUNCTION bodies ──
      [b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80.., ..] => {
        let end = s.iter()
          .position(|&b| !(b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80))
          .unwrap_or(s.len());
        let ident = &s[..end];
        s = &s[end..];
        track_begin_end(ident, &mut identifier_count, &mut identifiers,
          paren_depth, &mut begin_depth);
      }
      _ => s = &s[1..],
    }
  }
  sql.len()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// If `s` starts with a valid `$tag$` delimiter, returns its byte length.
fn dollar_tag_len(s: &[u8]) -> Option<usize> {
  debug_assert_eq!(s[0], b'$');
  let close = s[1..].iter().position(|&b| b == b'$')?; // TODO fix full scan
  let body = &s[1..1 + close];
  let valid = body.is_empty()
    || ((body[0].is_ascii_alphabetic() || body[0] == b'_' || body[0] >= 0x80)
      && body[1..].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80));
  valid.then_some(close + 2)
}

/// Scans a nested block comment. Called *after* the opening `/*` is consumed.
fn scan_block_comment(s: &mut &[u8]) {
  let mut depth = 1i32;
  while depth > 0 {
    match *s {
      [b'/', b'*', tail @ ..] => { depth += 1; *s = tail; }
      [b'*', b'/', tail @ ..] => { depth -= 1; *s = tail; }
      [_, tail @ ..] => *s = tail,
      [] => break,
    }
  }
}

/// Scans past a single-quoted string. Called *after* the opening `'` is consumed.
///
/// Handles `''` embedded quote, `\'` when `allow_backslash`, and SQL string
/// continuation (`'  \n  '` across lines).
fn scan_squoted(s: &mut &[u8], allow_backslash: bool) {
  loop {
    match *s {
      [] => break,
      [b'\\', _, tail @ ..] if allow_backslash => *s = tail,
      [b'\'', b'\'', tail @ ..] => *s = tail,
      [b'\'', tail @ ..] => {
        *s = tail;
        // SQL continuation: whitespace containing ≥1 newline, then another '
        let saved = *s;
        let mut has_nl = false;
        loop {
          match *s {
            [b'\n' | b'\r', tail @ ..] => { has_nl = true; *s = tail; }
            [b' ' | b'\t' | b'\x0c' | b'\x0b', tail @ ..] => *s = tail,
            // a line comment between two parts is also allowed
            [b'-', b'-', tail @ ..] => {
              *s = tail;
              while let [b, tail @ ..] = *s {
                if matches!(b, b'\n' | b'\r') { break; }
                *s = tail;
              }
            }
            _ => break,
          }
        }
        if has_nl && matches!(*s, [b'\'', ..]) {
          *s = &s[1..]; // eat continuation opening quote, keep scanning
        } else {
          *s = saved; // string ended at the first quote
          break;
        }
      }
      [_, tail @ ..] => *s = tail,
    }
  }
}

/// Scans past a double-quoted identifier. Called *after* the opening `"` is consumed.
fn scan_dquoted(s: &mut &[u8]) {
  loop {
    match *s {
      [] => break,
      [b'"', b'"', tail @ ..] => *s = tail, // "" embedded quote
      [b'"', tail @ ..] => { *s = tail; break; }
      [_, tail @ ..] => *s = tail,
    }
  }
}

/// Skips whitespace and any leading comments (line or block) repeatedly.
fn skip_ws_comments(s: &mut &[u8]) {
  loop {
    while let [b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b', tail @ ..] = *s {
      *s = tail;
    }
    match *s {
      [b'-', b'-', tail @ ..] => {
        *s = tail;
        while let [b, tail @ ..] = *s {
          if matches!(b, b'\n' | b'\r') { break; }
          *s = tail;
        }
      }
      [b'/', b'*', tail @ ..] => { *s = tail; scan_block_comment(s); }
      _ => break,
    }
  }
}

/// Maintains the `identifiers` heuristic and `begin_depth` counter.
///
/// Mirrors psqlscan's logic for detecting semicolons that are inside
/// `BEGIN`/`END` bodies of `CREATE [OR REPLACE] FUNCTION/PROCEDURE`.
fn track_begin_end(
  ident: &[u8],
  count: &mut usize,
  ids: &mut [u8; 4],
  paren_depth: i32,
  begin_depth: &mut i32,
) {
  if *count == 0 { *ids = [0u8; 4]; }

  let first = ident[0].to_ascii_lowercase();
  let tracked = match first {
    b'c' => ident.eq_ignore_ascii_case(b"create"),
    b'o' => ident.eq_ignore_ascii_case(b"or"),
    b'r' => ident.eq_ignore_ascii_case(b"replace"),
    b'f' => ident.eq_ignore_ascii_case(b"function"),
    b'p' => ident.eq_ignore_ascii_case(b"procedure"),
    _ => false,
  };
  if tracked && *count < 4 { ids[*count] = first; }
  *count += 1;

  // CREATE FUNCTION/PROCEDURE  →  ids = [c, f|p, _, _]
  // CREATE OR REPLACE FUNCTION →  ids = [c, o, r, f|p]
  let in_create_func = matches!(
    ids,
    [b'c', b'f' | b'p', ..] | [b'c', b'o', b'r', b'f' | b'p']
  );

  if in_create_func && paren_depth == 0 {
    if ident.eq_ignore_ascii_case(b"begin") {
      *begin_depth += 1;
    } else if ident.eq_ignore_ascii_case(b"case") && *begin_depth >= 1 {
      *begin_depth += 1; // CASE...END also uses END
    } else if ident.eq_ignore_ascii_case(b"end") && *begin_depth > 0 {
      *begin_depth -= 1;
    }
  }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn basic() {
    let sql = br"SELECT 1; SELECT 2;";
    assert_eq!(statement_boundary(sql), 10);
  }

  #[test]
  fn no_trailing_semicolon() {
    let sql = br"SELECT 1";
    assert_eq!(statement_boundary(sql), sql.len());
  }

  #[test]
  fn paren_depth() {
    let sql = br"SELECT (1; 2); _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn string() {
    let sql = br"SELECT 'hello;world\'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn e_string() {
    let sql = br"SELECT E'hello\';world'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn e_string_continuation() {
    let sql = br"SELECT e'hello'
      'world\';'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn n_string() {
    let sql = br"SELECT n';\'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn n_string_continuation() {
    let sql = br"SELECT n'hello'
      '\'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

    #[test]
  fn u_string() {
    let sql = br"SELECT u&';\'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn u_string_continuation() {
    let sql = br"SELECT u'hello'
      '\'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn double_quoted_identifier() {
    let sql = br#"SELECT "a;b"; _"#;
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn dollar_quote_empty_tag() {
    let sql = br"SELECT $$a;b\$$; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn dollar_quote() {
    let sql = br"SELECT $tag_$a;b\$tag_$; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn parameter() {
    let sql = br"SELECT $1; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn line_comment() {
    let sql = br"-- ;
      SELECT 1; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn block_comment() {
    let sql = br"/* /* */ ; */ SELECT 1; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn begin_atomic() {
    let sql = br"
      create or replace function hello(lang text)
      returns text
      begin atomic;
        select case lang
          when 'fr' then 'bonjour;'
          else $$hello;$$
        end;
      end;
      _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn consume_trailing_whitespace() {
    // Consume trailing comments so the next statement starts with an actual command.
    // This way, if the next statement fails without a position field,
    // the resulting position will point to the beginning of the actual command
    // rather than the leading comment.
    let sql = br"
      SELECT 1;
      -- line comment
      /* block comment */

      _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn empty_input() {
    assert_eq!(statement_boundary(b""), 0);
  }
}
