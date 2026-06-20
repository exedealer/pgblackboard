import { editor } from '../_vendor/monaco.js';

const methods = {
  _render() {
    const store = this.get_store();
    const { frame_idx, row_idx, col_idx } = store.get_selected_rowcol();
    const { value } = store.get_datum(frame_idx, row_idx, col_idx);

    return {
      tag: 'div',
      class: 'datum',
      'data-null': value === null || null,
      'data-unset': value === undefined || null,
    };
  },
  _mounted() {
    this._model = editor.createModel('');

    this._editor = editor.create(this.$el, {
      // https://microsoft.github.io/monaco-editor/docs.html#interfaces/editor.IStandaloneEditorConstructionOptions.html
      model: this._model,
      automaticLayout: true,
      tabSize: 2,
      fontSize: 14,
      lineHeight: 20,
      fontFamily: 'Roboto Mono',
      fontWeight: '400',
      minimap: { enabled: false },
      stickyScroll: { enabled: false },
      renderLineHighlight: 'none',
      // https://github.com/microsoft/monaco-editor/issues/3829
      // bracketPairColorization: { enabled: false },
      'bracketPairColorization.enabled': false,
      padding: { top: 8, bottom: 8 },
      lineNumbers: 'off',
      scrollbar: {
        horizontalScrollbarSize: 8,
        verticalScrollbarSize: 8,
        useShadows: false,
      },
      wordWrap: 'on',
      wrappingIndent: 'same',
      unicodeHighlight: { ambiguousCharacters: false },
      unusualLineTerminators: 'off',
    });

    this._editor.onDidFocusEditorText(this._on_focus);
    this._editor.onDidBlurEditorText(this._on_blur);
    this._editor.onKeyUp(this._on_keyup);

    const blank_el = this.$el.ownerDocument.createElement('div');
    blank_el.className = 'datum-blank';
    this._editor.applyFontInfo(blank_el);
    this._editor.addContentWidget({
      getId: _ => 'editor.widget.blank',
      getDomNode: _ => blank_el,
      getPosition: _ => ({
        position: { lineNumber: 1, column: 1 },
        preference: [editor.ContentWidgetPositionPreference.EXACT],
      }),
    });

    window.debug_editor_datum = this._editor;

    this.$watch(
      _ => this.get_store().get_selected_rowcol(),
      this._watch_selected_rowcol,
      { immediate: true },
    );
  },
  _unmounted() {
    this._editor.dispose();
    this._model.dispose();
  },
  _get_broadcast_listeners() {
    return { 'req_datum_focus': this._on_req_datum_focus };
  },
  _watch_selected_rowcol({ frame_idx, row_idx, col_idx }) {
    // TODO special view when no selected cell
    // TODO special case when inserting row, empty_val should use default value

    const store = this.get_store();
    // TODO fix no refresh on Revert changes
    const { type, att_name, att_notnull, original_value, value } =
      store.get_datum(frame_idx, row_idx, col_idx);
    const syntax = this._get_language_of_pgtype(type);
    const model = editor.createModel(value || '', syntax);

    this._model?.dispose(); // TODO async concurency
    this._model = model;
    this._editor.setModel(this._model);
    this._editor.updateOptions({ readOnly: !att_name });
    // if (typeOid == 3802) {
    //   await this._editor.getAction('editor.action.formatDocument').run();
    // }

    const blank = (
      original_value === undefined ? undefined :
      att_notnull ? '' :
      null // TODO how to set '' to nullable column?
    );
    this._model.onDidChangeContent(_ => {
      // TODO avoid getValue() on each keypress.
      // We need to show only the starting piece in the table cell
      const new_val = this._model.getValue() || blank;
      store.edit_row(frame_idx, row_idx, col_idx, new_val);
    });
  },
  _on_req_datum_focus() {
    const full_range = this._model.getFullModelRange();
    this._editor.setSelection(full_range);
    this._editor.focus();
  },
  _on_focus() {
    this.get_store().sync_datum_focused(true);
  },
  _on_blur() {
    this.get_store().sync_datum_focused(false);
  },
  _on_keyup(e) {
    if (e.code == 'Escape' && !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey) {
      this.$broadcast('req_cell_focus');
    }
  },
  _get_language_of_pgtype(type) {
    if (type == 'json' || type == 'jsonb') return 'json';
    if (type == 'xml') return 'xml';
    return null;
  },

  /** @returns {import('../store/store.js').Store} */
  get_store() {
    return this.$store;
  },
};

export default {
  methods,
};
