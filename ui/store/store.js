import { editor, Uri } from '../_vendor/monaco.js';
import { hexwkb2t } from './wkb2t.js';

export class Store {
  dark = { sys: false, on: null };
  timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  panes = { left: .2, right: .6, out: 1, map: 0 };
  auth = {
    pending: false,
    done: false,
    error: '',
    u: '',
    token: '',
  };

  // TODO single field
  selected_treenode_path = null;
  selected_draft_id = null;

  tree = {
    children: {
      nodes: [],
      error: null,
      ready: false,
    },
  };
  // TODO track run_count for drafts, gc least used
  drafts_kv = {};
  /** @type {string[]} */
  stored_draft_ids = [];
  /** @type {Record<string, true>} */
  dirty_draft_ids = null;
  datum_focused = false;
  out = {
    /**
     * @type {{
     *  selected_col_idx: number,
     *  rel_name: string | null,
     *  cols: {
     *    name: string,
     *    type: string,
     *    att_name: string | null,
     *    att_key: boolean,
     *    att_notnull: boolean,
     *    is_geom: boolean,
     *    hue: number,
     *    show_on_map: boolean,
     *    width: number,
     *  }[],
     *  rows: {
     *    original: string[] | null,
     *    modified: { delete: boolean } & Record<number, string> | null,
     *  }[],
     * }[]}
     */
    frames: [], // TODO frames -> tables
    messages: [],
    /** @type {number} */
    selected_frame_idx: null,
    /** @type {number} */
    selected_row_idx: null,
    /** @type {AbortController} */
    aborter: null,
    loading: false,
    suspended: null,
    /** @type {string} */
    db: null,

    // TODO draft_ver: null,
    // check if draft modified before
  };

  show_sat = false;

  toggle_sat() {
    this.show_sat = !this.show_sat;
  }

  resize_panes(update) {
    Object.assign(this.panes, update);
  }

  sync_datum_focused(value) {
    this.datum_focused = value;
  }

  async auth_submit(u, password) {
    this.auth = {
      pending: true,
      ok: false,
      error: null,
      u, // TODO u -> user
      token: null,
    };
    const auth = this.auth;
    try {

      const resp = await fetch('api/auth', {
        method: 'POST',
        headers: { 'accept': 'application/json' },
        body: new URLSearchParams({ user: u, password }),
      });
      // TODO use resp.body for error message for non-200 response
      if (!resp.ok) throw Error(`${resp.status} ${resp.statusText}`, { cause: resp });
      const { token } = await resp.json() || 0;
      // await new Promise(resolve => setTimeout(resolve, 3000));
      auth.token = token;
      // TODO concurent store mutation if multiple parallel .auth() called
      await this._load_tree_and_drafts();

      auth.ok = true;
    } catch (ex) {
      console.error(ex);
      auth.error = String(ex);
    } finally {
      auth.pending = false;
    }
  }

  async _load_tree_and_drafts() {
    await this.tree_toggle([]);
    // TODO handle this.$store.tree.children.error;

    const initial_draft_id = this._add_draft(
      `\\connect postgres\n\n` +
      `SELECT to_jsonb(sa), * \n` +
      `FROM pg_stat_activity sa;\n`,
    );
    this._unset_selected_draft();
    this.selected_draft_id = initial_draft_id;

    this._load_drafts();
    setInterval(_ => this._flush_drafts(), 10e3);
    // TODO visibilitychange
    globalThis.addEventListener('unload', _ => this._flush_drafts());
  }

  _load_drafts() {
    const storage = globalThis.localStorage; // TODO dry dep injection
    this.stored_draft_ids = (
      Object.keys(storage)
      .filter(k => /^pgblackboard_draft_/.test(k))
      .sort()
      .reverse()
    );
    for (const id of this.stored_draft_ids) {
      this._add_draft(storage.getItem(id), id);
    }
  }

  _flush_drafts() {
    // TODO gc drafts
    if (!this.dirty_draft_ids) return;
    const storage = globalThis.localStorage; // TODO dry dep injection
    for (const id in this.dirty_draft_ids) {
      if (this.stored_draft_ids.includes(id)) {
        const value = editor.getModel(this.get_draft_uri(id)).getValue();
        // TODO handle QuotaExceededError
        storage.setItem(id, value);
      } else {
        storage.removeItem(id);
      }
    }
    this.dirty_draft_ids = null;
  }

  _add_draft(content, draft_id) {
    draft_id ||= (
      'pgblackboard_draft_' // legacy v2 prefix
      + '_' // v2 suffix is not sortable because not zero padded, so push v3 drafts upper
      + Date.now().toString(16).padStart(16, '0')
    );
    const editor_model = editor.createModel(content, 'sql', this.get_draft_uri(draft_id));
    this.drafts_kv[draft_id] = {
      id: draft_id,
      loading: false,
      caption: '',
      // TODO pass selection_range to .run() action.
      // This was done to be able run selection in non-editor component.
      // But currently RUN button does only full run.
      // Or we can just store selection range globaly.
      // No need to store selection on draft item.
      cursor_pos: 0,
      cursor_len: 0,
    };
    const draft = this.drafts_kv[draft_id];
    editor_model.onDidChangeContent(update_caption);
    update_caption();
    return draft_id;

    function update_caption() {
      const pos = editor_model.getPositionAt(256);
      const head = editor_model.getValueInRange({
        startLineNumber: 0,
        startColumn: 0,
        endLineNumber: pos.lineNumber,
        endColumn: pos.column,
      });
      const { sql } = extract_dbname_from_sql(head);
      draft.caption = sql.trim();
      // draft.caption = head.replace(/^\s*\\connect[\s\t]+[^\n]+\n/, '\u2026 ');
    }
  }

  get_draft_uri(draft_id) {
    return Uri.parse('//pgbb/' + draft_id);
  }

  save_selected_draft() {
    this.dirty_draft_ids ||= {};
    this.dirty_draft_ids[this.selected_draft_id] = true;
    if (!this.stored_draft_ids.includes(this.selected_draft_id)) {
      this.stored_draft_ids.unshift(this.selected_draft_id);
      this.selected_treenode_path = null;
    }
  }

  async tree_toggle(path) {
    const node = path.reduce(({ children }, idx) => children.nodes[idx], this.tree);
    const should_collapse = node.children.ready != false;
    node.children = { nodes: [], error: null, ready: false };
    if (should_collapse) return;
    const { db, children, ntype, noid, ntid } = node;
    const { u: user, token } = this.auth;
    try {
      children.ready = null; // loading
      const args = (
        Object.entries({ user, db, ntype, noid, ntid })
        .filter(([_, v]) => v != null)
      );
      const resp = await fetch('api/tree?' + new URLSearchParams(args), {
        method: 'POST',
        headers: { 'x-pgbb-auth': token },
      });
      if (!resp.ok) throw Error('api/tree error', { cause: resp });
      const result = await resp.json();
      // await new Promise(res => setTimeout(res, 2000));
      for (const cn of result) {
        cn.children = { nodes: [], error: null, ready: false };
        children.nodes.push(cn);
      }
    } catch (ex) {
      children.error = String(ex);
    } finally {
      children.ready = true;
    }
  }

  // TODO set_selected_treenode
  async tree_select(path) {
    const draft_id = this._add_draft('-- loading --');
    const draft = this.drafts_kv[draft_id];
    draft.loading = true;
    const editor_model = editor.getModel(this.get_draft_uri(draft_id));
    this._unset_selected_draft();
    this.selected_draft_id = draft_id;
    this.selected_treenode_path = path;
    const node = path.reduce(
      ({ children }, idx) => children.nodes[idx],
      this.tree,
    );
    const { db, ntype, noid, ntid } = node;
    const { u: user, token } = this.auth;
    const args = (
      Object.entries({ user, db, ntype, noid, ntid })
      .filter(([_, v]) => v != null)
    );

    let content;
    try {
      const resp = await fetch('api/defn?' + new URLSearchParams(args), {
        method: 'POST',
        headers: { 'x-pgbb-auth': token },
      });
      if (!resp.ok) throw Error('api/defn error', { cause: resp });
      content = await resp.text();
    } catch (ex) {
      content = `/* ${ex} */\n`;
    }
    // TODO indicate dead treenode when treenode not found
    if (!editor_model.isDisposed()) {
      editor_model.setValue(content);
    }
    draft.loading = false;
  }

  async _api(api, qs, body) {
    qs = JSON.parse(JSON.stringify(qs)); // rm undefined
    const resp = await fetch('?' + new URLSearchParams({ api, ...qs }), {
      method: 'POST',
      headers: {
        'content-type': 'application/json; charset=utf-8',
        'x-pgbb-auth': this.auth?.token,
      },
      body: JSON.stringify(body),
    });
    if (!resp.ok) throw Error(`${resp.status} ${resp.statusText}`);
    return resp.json();
  }

  get selected_draft() {
    return this.drafts_kv[this.selected_draft_id];
  }

  set_code_cursor(cursor_pos, cursor_len) {
    Object.assign(this.selected_draft, { cursor_pos, cursor_len });
  }

  _unset_selected_draft() {
    if (
      this.selected_draft_id in this.drafts_kv &&
      !this.stored_draft_ids.includes(this.selected_draft_id)
    ) {
      delete this.drafts_kv[this.selected_draft_id];
      editor.getModel(this.get_draft_uri(this.selected_draft_id)).dispose();
    }
    this.selected_draft_id = null;
  }
  set_selected_draft(draft_id) {
    this._unset_selected_draft(draft_id);
    this.selected_draft_id = draft_id;
    this.selected_treenode_path = null;
  }

  rm_draft(draft_id) {
    const idx = this.stored_draft_ids.indexOf(draft_id);
    this.stored_draft_ids.splice(idx, 1);
    delete this.drafts_kv[draft_id];
    editor.getModel(this.get_draft_uri(draft_id)).dispose();
    this.dirty_draft_ids ||= {};
    this.dirty_draft_ids[draft_id] = true;
    if (draft_id == this.selected_draft_id) {
      // TODO
      this.selected_draft_id = null;
    }
  }

  resize_col(frame_idx, col_idx, width) {
    this.out.frames[frame_idx].cols[col_idx].width = width;
  }

  toggle_show_on_map(frame_idx, col_idx, value) {
    this.out.frames[frame_idx].cols[col_idx].show_on_map = value;
  }

  // TODO set_selected_row
  set_selected_rowcol(frame_idx, row_idx, col_idx) {
    this.out.selected_frame_idx = frame_idx;
    this.out.selected_row_idx = row_idx;
    if (col_idx != null) {
      this.out.frames[frame_idx].selected_col_idx = col_idx;
    }
  }

  get_selected_rowcol() {
    return {
      frame_idx: this.out.selected_frame_idx,
      row_idx: this.out.selected_row_idx,
      col_idx: this.out.frames[this.out.selected_frame_idx]?.selected_col_idx,
    };
  }

  get_datum(frame_idx, row_idx, col_idx) {
    const frame = this.out.frames[frame_idx];
    const { type, att_name, att_notnull } = frame?.cols?.[col_idx] || 0;
    const { original, modified } = frame?.rows?.[row_idx] || 0;
    const original_value = original?.[col_idx];
    const { [col_idx]: value = original_value } = modified || 0;
    return { type, att_name, att_notnull, original_value, value };
  }

  delete_row(frame_idx, row_idx) {
    this.out.frames[frame_idx].rows[row_idx].modified = { delete: true };
  }

  revert_row(frame_idx, row_idx) {
    const { rows } = this.out.frames[frame_idx];
    const row = rows[row_idx];
    row.modified = null;
    if (!row.original) {
      rows.splice(row_idx, 1);
    }
  }

  edit_row(frame_idx, row_idx, col_idx, new_value) {
    const rows = this.out.frames[frame_idx].rows;
    rows[row_idx] ||= { original: null, modified: null };
    const row = rows[row_idx];
    row.modified ||= {};
    // TODO same column can be selected multiple times
    // so att_name should be used instead of col_idx to identify updated datum
    row.modified[col_idx] = new_value;
  }

  get_changes_num() {
    let count = 0;
    for (const { rows } of this.out.frames) {
      // TODO avoid full scan
      for (const { modified } of rows) {
        if (modified) {
          count++;
        }
      }
    }
    return count;
  }

  dump_changes() {
    let script = `\\connect ${this.out.db}\n\n`;

    for (const frame of this.out.frames) {
      const key_idxs = frame.cols.map((col, i) => col.att_key && i).filter(Number.isInteger);
      const key_names = tuple_expr(key_idxs.map(i => frame.cols[i].att_name));

      const delete_keys = [];
      const update_keys = [];
      const update_stmts = [];
      const insert_stmts = [];
      for (const { original, modified } of frame.rows) {
        if (!modified) continue;

        // INSERT
        if (!original) {
          const cols = [];
          const vals = [];
          for (const [col_idx, col] of frame.cols.entries()) {
            const val = modified[col_idx];
            if (val === undefined) continue;
            vals.push(literal(val));
            cols.push(col.att_name);
          }
          insert_stmts.push(
            `INSERT INTO ${frame.rel_name} (${cols.join(', ')})\n` +
            `VALUES (${vals.join(', ')});\n\n`,
          );
          continue;
        }

        const key_vals = tuple_expr(key_idxs.map(i => literal(original[i])));

        // DELETE
        if (modified.delete) {
          delete_keys.push(key_vals);
          continue;
        }

        // UPDATE
        const set_entries = [];
        for (const [col_idx, col] of frame.cols.entries()) {
          const upd_value = modified[col_idx];
          if (upd_value === undefined) continue;
          set_entries.push(`${col.att_name} = ${literal(upd_value)}`);
        }
        update_stmts.push(
          `UPDATE ${frame.rel_name}\n` +
          `SET ${set_entries.join('\n  , ')}\n` +
          `WHERE ${key_names} = ${key_vals};\n\n`,
        );
        update_keys.push(key_vals);
      }
      if (delete_keys.length) {
        script += (
          `DELETE FROM ${frame.rel_name}\n` +
          `WHERE ${key_names} IN (\n  ${delete_keys.join(',\n  ')}\n);\n\n`
        );
      }
      if (update_stmts.length) {
        script += update_stmts.join('');
        script += (
          `SELECT * FROM ${frame.rel_name}\n` +
          `WHERE ${key_names} IN (\n  ${update_keys.join(',\n  ')}\n);\n\n`
        );
      }
      script += insert_stmts.join('');
    }

    const draft_id = this._add_draft(script);
    this._unset_selected_draft();
    this.selected_draft_id = draft_id;
    this.selected_treenode_path = null;

    // TODO clear edits

    function literal(s) {
      return s == null ? 'NULL' : `'${s.replace(/'/g, `''`)}'`;
    }
    function tuple_expr(arr) {
      const joined = arr.join(', ');
      return arr.length > 1 ? `(${joined})` : joined;
    }
  }

  is_dark(fallback = this.dark.sys) {
    return this.dark.on ?? fallback;
  }
  set_dark_sys(value) {
    this.dark.sys = value;
  }
  toggle_dark() {
    this.dark.on = !this.is_dark();
  }

  can_abort() {
    return Boolean(this.out.aborter);
  }

  abort() {
    this.out.aborter.abort();
  }

  can_wake() {
    return Boolean(this.out.suspended);
  }

  async wake() {
    const id = this.out.suspended.wake_id;
    const { u: user, token } = this.auth;
    const headers = { 'x-pgbb-auth': token };
    const arg = new URLSearchParams({ user, id });
    const resp = await fetch('api/wake?' + arg, { method: 'POST', headers });
    if (!resp.ok) throw Error('wake error', { cause: resp });
  }

  can_run() {
    return !this.out.loading && !this.selected_draft?.loading;
  }

  async run({ selected } = 0) {
    this.out = {
      db: null,
      frames: [],
      messages: [],
      selected_frame_idx: null,
      selected_row_idx: null,
      aborter: new AbortController(),
      loading: true,
      suspended: null,
    };
    const out = this.out;

    let stmt_pos = 0;

    try {
      const tz = this.timezone;
      const draft = this.selected_draft;
      const { cursor_pos, cursor_len } = draft;
      const editor_model = editor.getModel(this.get_draft_uri(this.selected_draft_id));
      const editor_text = editor_model.getValue();

      let { db, sql } = extract_dbname_from_sql(editor_text);
      if (db == null) {
        throw Error(`missing \\connect meta-command in first line`);
      }
      out.db = db;

      if (selected) {
        const [from, to] = [cursor_pos, cursor_pos + cursor_len].sort((a, b) => a - b);
        sql = '\n'.padStart(from, ' ') + sql.slice(from, to);
      }

      const { u, token } = this.auth;
      const qs = new URLSearchParams({ user: u, db, tz });
      const resp = await fetch('api/run?' + qs, {
        method: 'POST',
        signal: out.aborter.signal,
        headers: {
          'x-pgbb-auth': token, // TODO dry
        },
        body: sql,
      });
      if (!resp.ok) {
        throw Error(`HTTP ${resp.status} ${resp.statusText}`, {
          cause: await resp.text(),
        });
      }

      const hues = sunflower(200 /* blue */);

      const msg_stream = (
        resp.body
        .pipeThrough(new TextDecoderStream())
        .pipeThrough(new LineSplitter())
      );
      for await (const lines of msg_stream)
      for (const line of lines) {
        const [tag, payload] = JSON.parse(line);
        out.suspended = null;
        switch (tag) {
          case 'start':
            stmt_pos = payload.position_utf16;
            break;
          case 'head':
            out.frames.push({
              rel_name: payload.rel_name,
              cols: payload.cols.map(col => ({
                ... col,
                width: 150, // TODO auto
                hue: -1,
                show_on_map: false,
                ... col.is_geom && {
                  hue: hues.next().value,
                  show_on_map: true,
                },
              })),
              selected_col_idx: Boolean(payload.cols.length) - 1,
              rows: [],
            });
            break;
          // TODO CopyData
          case 'row':
            const frame = out.frames.at(-1);
            for (let col_idx = frame.cols.length; col_idx--;) {
              const datum = payload[col_idx];
              if (datum == null) continue;
              const col = frame.cols[col_idx];
              try {
                payload[col_idx] = prettify_datum(col, datum);
              } catch (ex) {
                // TODO handle datum transformation error
              }
            }
            frame.rows.push({
              original: Object.freeze(payload),
              modified: null,
            });
            // select first row in first non empty table
            // TODO select first frame regardless of rows count? (more predictable behavior)
            if (out.selected_frame_idx == null) {
              out.selected_frame_idx = out.frames.length - 1;
              out.selected_row_idx = 0;
            }
            break;
          case 'complete':
          case 'error':
          case 'notice':
            let position = stmt_pos;
            if (isFinite(payload.position)) {
              let err_pos_1based_codepoints = Number(payload.position);
              for (const ch of sql.slice(stmt_pos)) {
                if (err_pos_1based_codepoints-- <= 1) break;
                position += ch.length;
              }
            }
            out.messages.push({ tag, position, payload });
            break;
          case 'suspended':
            out.suspended = payload;
            break;
        }
      }
    } catch (ex) {
      out.messages.push({
        tag: 'error',
        position: stmt_pos,
        payload: {
          severity_en: 'ERROR',
          severity: 'ERROR', // TODO non localized
          // code: 'E_PGBB_FRONTEND',
          message: String(ex),
          detail: ex?.stack, // TODO .cause
        },
      });
    } finally {
      out.loading = false;
      out.aborter = null;
      out.suspended = null;
    }
  }
}

class LineSplitter extends TransformStream {
  constructor() {
    let partial = '';
    super({
      async transform(chunk, ctl) {
        const lines = (partial + chunk).split('\n');
        partial = lines.pop(); // last line is not terminated, keep it
        ctl.enqueue(lines);
      },
      // TODO flush incomplete line?
    });
  }
}

// TODO move to backend
// - return 'db' message from stream
// - skip /^\\.*/ in draft title
// - how to do `run selection` if we keep dbname in script?
function extract_dbname_from_sql(sql) {
  let db;
  sql = sql.replace(/^\s*\\connect\s+(.*)/, (line, arg) => {
    arg = arg.trim();
    if (/^"/.test(arg)) { // unquote
      arg = arg.replace(/^"|"$|"(?=")/g, '');
    }
    db = arg;
    return ' '.repeat(line.length);
  });
  return { db, sql };
}

function prettify_datum({ type, is_geom }, datum) {
  if (type == 'jsonb') return json_pretty(datum);
  if (is_geom) return hexwkb2t(datum);
  return datum;
}

function json_pretty(/** @type {string} */ input) {
  const indent = '  ';
  const json_re = /\s*({|}|\[|]|,|:|true|false|null|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|"(?:\\.|[^"])*")\s*/yg;
  let level = 0;

  return input.replace(json_re, (_, token) => {
    switch (token) {
      case '{':
      case '[':
        return token + '\n' + indent.repeat(++level);
      case '}':
      case ']':
        return '\n' + indent.repeat(--level) + token;
      case ',':
        return token + '\n' + indent.repeat(level);
      case ':':
        return token + ' ';
      default:
        return token;
    }
  });
}

function * sunflower(offset = 0) {
  // The idea is to dynamically assign colors to layers in such a way
  // that color contrast is high when there are few layers,
  // and gradually degrades as the number of layers increases.
  // https://en.wikipedia.org/wiki/Golden_angle
  const golden_angle = 180 * (3 - 5 ** .5);
  for (let i = 0;; i++) {
    yield (offset + i * golden_angle) % 360;
  }
  // TODO fix bad contrast when 245 (deep blue) on black bg
  // try OKLCH
}
