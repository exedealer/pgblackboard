/// Returns the number of bytes consumed by the first SQL statement in `sql`,
/// including the terminating `;` and any trailing whitespace/comments after it.
/// If there is no `;` the full input length is returned (unterminated statement).
pub fn statement_boundary(mut sql: &[u8]) -> usize {
  let before_sql = sql;
  scan_statement(&mut sql);
  before_sql.len() - sql.len()
}

fn scan_statement(sql: &mut &[u8]) {
  let mut paren_depth: u32 = 0;
  let mut begin_depth: u32 = 0;
  let mut idents = [0u8; 4];
  let mut idents_count = 0;

  *sql = loop {
    if checked_scan(sql, scan_line_comment)
      || checked_scan(sql, scan_block_comment)
      || checked_scan(sql, scan_double_quoted)
      || checked_scan(sql, scan_single_quoted)
      || checked_scan(sql, scan_dollar_quoted)
    {
      continue;
    }

    if let Some(ident) = scan_ident(sql) {
      // https://github.com/postgres/postgres/blob/f5cc81719e6da4cbdb1f797c48b693e91018153a/src/fe_utils/psqlscan.l#L917
      // We need to track if we are inside a BEGIN .. END block
      // in a function definition, so that semicolons contained
      // therein don't terminate the whole statement.  Short of
      // writing a full parser here, the following heuristic
      // should work.  First, we track whether the beginning of
      // the statement matches CREATE [OR REPLACE]
      // {FUNCTION|PROCEDURE}
      let tracked: [&[u8]; _] =
        [b"create", b"or", b"replace", b"function", b"procedure"];
      if let Some(slot) = idents.get_mut(idents_count) {
        idents_count += 1;
        let key = ident[0].to_ascii_lowercase();
        *slot = tracked.contains(&ident).then_some(key).unwrap_or_default();
      }

      let in_create_func = matches!(
        idents,
        | [b'c', b'f' | b'p', ..] // CREATE FUNCTION|PROCEDURE
        | [b'c', b'o', b'r', b'f' | b'p'] // CREATE OR REPLACE FUNCTION|PROCEDURE
      );

      if in_create_func && paren_depth == 0 {
        if ident.eq_ignore_ascii_case(b"begin") {
          begin_depth += 1;
        } else if ident.eq_ignore_ascii_case(b"case") && begin_depth >= 1 {
          begin_depth += 1; // CASE...END also uses END
        } else if ident.eq_ignore_ascii_case(b"end") && begin_depth > 0 {
          begin_depth -= 1;
        }
      }
      continue;
    }

    *sql = match sql {
      [] => break sql,
      [b';', tail @ ..] if paren_depth == 0 && begin_depth == 0 => break tail,
      [b'(', tail @ ..] => {
        paren_depth += 1;
        tail
      }
      [b')', tail @ ..] => {
        paren_depth = paren_depth.saturating_sub(1);
        tail
      }
      [_, tail @ ..] => tail,
    };
  };

  while checked_scan(sql, scan_line_comment)
    || checked_scan(sql, scan_block_comment)
    || checked_scan(sql, scan_whitespaces)
  {}
}

fn checked_scan(s: &mut &[u8], scanner: fn(&mut &[u8])) -> bool {
  let before_scan = *s;
  scanner(s);
  s.len() != before_scan.len()
}

fn scan_whitespaces(s: &mut &[u8]) {
  while let [
    b'\n' | b'\r' | b'\t' | b'\x0b' | b'\x0c' | b'\x20',
    tail @ ..,
  ] = s
  {
    *s = tail;
  }
}

fn scan_line_comment(s: &mut &[u8]) {
  // https://github.com/postgres/postgres/blob/f5cc81719e6da4cbdb1f797c48b693e91018153a/src/fe_utils/psqlscan.l#L154
  if let Some(tail) = s.strip_prefix(b"--") {
    *s = tail;
    while !matches!(s.split_off_first(), Some(b'\n' | b'\r') | None) {}
  }
}

fn scan_block_comment(s: &mut &[u8]) {
  // TODO? fix scans /* *//*  */ in one scan
  let mut depth = 0_u32;
  loop {
    *s = match s {
      [b'/', b'*', tail @ ..] => {
        depth += 1;
        tail
      }
      [b'*', b'/', tail @ ..] if depth > 0 => {
        depth -= 1;
        tail
      }
      [_, tail @ ..] if depth > 0 => tail,
      _ => break,
    };
  }
}

fn scan_double_quoted(s: &mut &[u8]) {
  let mut inside = false;
  loop {
    *s = match s {
      [b'"', tail @ ..] => {
        inside = !inside;
        tail
      }
      [_, tail @ ..] if inside => tail,
      _ => return,
    };
  }
}

fn scan_single_quoted(s: &mut &[u8]) {
  let slash_escape;
  (*s, slash_escape) = match s {
    [b'\'', tail @ ..] => (tail, false),
    [b'e' | b'E', b'\'', tail @ ..] => (tail, true),
    _ => return,
  };

  loop {
    *s = loop {
      *s = match s {
        [b'\\', _, tail @ ..] if slash_escape => tail,
        [b'\'', b'\'', tail @ ..] => tail,
        [b'\'', tail @ ..] => break tail, // literal interrupted
        [_, tail @ ..] => tail,
        [] => return,
      };
    };

    // scan line comments and whitespaces
    let mut has_nl = false;
    let mut in_comment = false;
    *s = loop {
      *s = match s {
        [b'\n' | b'\r', tail @ ..] => {
          in_comment = false;
          has_nl = true;
          tail
        }
        [_, tail @ ..] if in_comment => tail,
        [b'-', b'-', tail @ ..] => {
          in_comment = true;
          tail
        }
        [b'\t' | b'\x0b' | b'\x0c' | b'\x20', tail @ ..] => tail,
        [b'\'', tail @ ..] if has_nl => break tail, // continue literal scan
        _ => return,
      }
    }
  }
}

fn scan_dollar_quoted(s: &mut &[u8]) {
  let tag;
  let is_dolq_cont =
    |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0o200;
  (tag, *s) = match s {
    // empty tag $$
    [b'$', b'$', ..] => s.split_at(1 + 1),
    // https://github.com/postgres/postgres/blob/f5cc81719e6da4cbdb1f797c48b693e91018153a/src/fe_utils/psqlscan.l#L228
    // ${dolq_start}{dolq_cont}*$
    [b'$', b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0o200.., tail @ ..]
      if let Some(cont_len) = tail.iter().position(|&b| !is_dolq_cont(b))
        && tail[cont_len] == b'$' =>
    {
      s.split_at(1 + 1 + cont_len + 1)
    }
    _ => return,
  };

  // scan content until tag
  *s = loop {
    *s = match s {
      [] => break s,
      _ if let Some(tail) = s.strip_prefix(tag) => break tail,
      [_, tail @ ..] => tail,
    };
  };
}

fn scan_ident<'a>(s: &mut &'a [u8]) -> Option<&'a [u8]> {
  // https://github.com/postgres/postgres/blob/f5cc81719e6da4cbdb1f797c48b693e91018153a/src/fe_utils/psqlscan.l#L276
  let [b'A'..=b'Z' | b'a'..=b'z' | b'_' | 0o200.., ..] = s else {
    return None;
  };
  let orig = *s;
  while let [
    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$' | 0o200..,
    tail @ ..,
  ] = s
  {
    *s = tail;
  }
  Some(&orig[..orig.len() - s.len()])
}

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
  fn parentheses() {
    let sql = br"SELECT (1; 2); _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn parentheses_unbalanced() {
    let sql = br"SELECT (1; 2)); _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn string() {
    let sql = br"SELECT 'hello;world\'; _";
    assert_eq!(statement_boundary(sql), sql.len() - 1);
  }

  #[test]
  fn string_incomplete() {
    let sql = br"SELECT 'hello; _";
    assert_eq!(statement_boundary(sql), sql.len());
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
  fn e_string_continuation_comment() {
    let sql = br"SELECT e'hello' -- comment
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
    let sql = br"SELECT n'hello;'
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
    let sql = br"SELECT u&'hello'
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
  fn dollar_ident() {
    let sql = br"SELECT 1 _$$; $$";
    assert_eq!(statement_boundary(sql), 14);
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
  fn line_comment_incomplete() {
    let sql = br"-- hello";
    assert_eq!(statement_boundary(sql), sql.len());
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
    // Consume trailing comments so the next statement starts
    // with an actual command. This way, if the next statement
    // fails without a position field, the resulting position
    // will point to the beginning of the actual command
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
