// TODO drafts hover preview
// TODO keyboard navigation

const xDrafts = {
  methods: {
    _render() {
      const { stored_draft_ids } = this.$store;
      return {
        tag: 'ul',
        class: 'drafts',
        'aria-label': 'Drafts',
        inner: stored_draft_ids.map(this._render_item, this),
      };
    },
    _render_item(draft_id) {
      return {
        tag: xDraftsItem,
        key: draft_id,
        draft_id,
      };
    },
  },
};

const xDraftsItem = {
  props: {
    draft_id: null,
  },
  methods: {
    _render() {
      const { draft_id } = this;
      const { selected_draft_id, drafts_kv } = this.$store;
      const { caption } = drafts_kv[draft_id];

      return {
        tag: 'li',
        class: 'drafts-item',
        'aria-selected': draft_id == selected_draft_id,
        inner: [
          {
            tag: 'a',
            class: 'drafts-link',
            inner: caption, // .replaceAll('\n', '\u21b5'),
            onClick: this.select,
          },
          {
            tag: 'button',
            class: 'drafts-delete',
            type: 'button',
            'aria-label': 'Delete draft',
            onClick: this.delete,
          },
        ],
      };
    },
    select() {
      this.$store.set_selected_draft(this.draft_id);
    },
    delete() {
      this.$store.rm_draft(this.draft_id);
    },
  },
};

export default xDrafts;
