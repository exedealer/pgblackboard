import { editor } from '../_vendor/monaco.js';

const methods = {
  _render() {
    return { tag: 'div', class: 'code' };
  },
  _mounted() {
    this._editor = editor.create(this.$el, {
      // https://microsoft.github.io/monaco-editor/docs.html#interfaces/editor.IStandaloneEditorConstructionOptions.html
      model: null,
      automaticLayout: true,
      tabSize: 2,
      fontSize: 14,
      lineHeight: 20,
      fontFamily: 'Roboto Mono',
      fontWeight: '400',
      minimap: { enabled: false },
      stickyScroll: { enabled: false },
      renderLineHighlight: 'none',
      useShadowDOM: true,
      overviewRulerLanes: 0,
      // https://github.com/microsoft/monaco-editor/issues/3829
      // bracketPairColorization: { enabled: false },
      'bracketPairColorization.enabled': false,
      padding: { top: 16, bottom: 16 },
      scrollbar: {
        horizontalScrollbarSize: 8,
        verticalScrollbarSize: 8,
        useShadows: false,
      },
    });

    this._decorations = this._editor.createDecorationsCollection();

    const ck_can_abort = this._editor.createContextKey('pgbb_can_abort', false);
    this.$watch(_ => this.$store.can_abort(), val => ck_can_abort.set(val), { immediate: true });

    const ck_can_run = this._editor.createContextKey('pgbb_can_run', false);
    this.$watch(_ => this.$store.can_run(), val => ck_can_run.set(val), { immediate: true });

    // https://microsoft.github.io/monaco-editor/typedoc/interfaces/editor.IActionDescriptor.html
    this._editor.addAction({
      id: 'pgbb.action.abort',
      label: 'Abort',
      contextMenuGroupId: 'navigation',
      contextMenuOrder: 0,
      precondition: 'pgbb_can_abort',
      run: _ => this.$store.abort({}),
    });
    this._editor.addAction({
      id: 'pgbb.action.run',
      label: 'Run',
      // contextMenuGroupId: 'navigation',
      // contextMenuOrder: 2,
      precondition: 'pgbb_can_run',
      run: _ => this.$store.run({}),
    });
    this._editor.addAction({
      id: 'pgbb.action.runSelection',
      label: 'Run Selection',
      contextMenuGroupId: 'navigation',
      contextMenuOrder: 4,
      precondition: 'pgbb_can_run && editorHasSelection && !editorHasMultipleSelections',
      run: _ => this.$store.run({ selected: true }),
    });

    this._editor.addAction({
      id: 'pgbb.action.toggle_sat',
      label: 'Toogle Satellite',
      run: _ => this.$store.toggle_sat(),
    });

    window.debug_editor = this._editor;

    this.$watch(
      _ => this.$store.selected_draft?.id,
      this.watch_draft_id,
      { immediate: true },
    );

    this.$watch(
      _ => this.$store.selected_draft?.loading || false,
      readOnly => this._editor.updateOptions({ readOnly }),
      { immediate: true },
    );

    this.$watch(
      _ => (
        this.$store.out.messages
        .filter(m => m.tag == 'error' && Number.isFinite(m.position))
      ),
      this.watch_errors,
      { immediate: true },
    );

    this._editor.onDidChangeCursorSelection(this.on_change_selection);
    this._editor.onDidChangeModelContent(this.on_change_content);
  },
  _unmounted() {
    this._editor.dispose();
  },

  watch_draft_id(draft_id) {
    if (!draft_id) return this._editor.setModel(null);
    const uri = this.$store.get_draft_uri(draft_id);
    const model = editor.getModel(uri);
    this._editor.setModel(model);
  },

  on_change_selection({ selection, secondarySelections }) {
    if (secondarySelections.length) {
      // TODO set no selection?
    }
    const model = this._editor.getModel();
    const cursor_pos = model.getOffsetAt(selection.getPosition());
    const cursor_end = model.getOffsetAt(selection.getSelectionStart());
    this.$store.set_code_cursor(cursor_pos, cursor_end - cursor_pos);
  },
  on_change_content() {
    const readonly = this._editor.getOption(editor.EditorOption.readOnly);
    if (!readonly) { // if edited by user
      this.$store.save_selected_draft();
    }
  },
  watch_errors(errors) {
    this._decorations.set(
      errors
      .map(e => this._editor.getModel().getPositionAt(e.position))
      .map(({ lineNumber, column }) => ({
        range: {
          startLineNumber: lineNumber,
          startColumn: column,
          endLineNumber: lineNumber,
          endColumn: column,
        },
        options: {
          isWholeLine: true,
          linesDecorationsClassName: 'code-error_decoration',
        },
      })),
    );
  },
};

export default {
  methods,
};
