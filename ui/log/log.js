/** @import { ComponentMixin } from '../main.js' */

const methods = {
  ... /** @type {ComponentMixin} */ ({}),

  _render() {
    const { messages } = this.$store.out;
    return {
      tag: 'div',
      class: 'log',
      'aria-label': 'log',
      inner: [
        {
          tag: 'p',
          class: 'log-header',
          inner: this._render_header_content(),
        },

        ...messages.map(m => ({
          tag: 'p',
          class: 'log-item',
          inner: this._render_item_content(m),
        })),
      ],
    };
  },
  _render_header_content() {
    const can_unsuspend = this.$store.can_unsuspend();
    const { suspended, loading, connecting, messages } = this.$store.out;
    const has_errors = messages.some(m => m.tag == 'error');

    // TODO render suspended.error

    if (suspended && suspended.reason == 'idle_in_transaction') {
      return [
        { tag: 'span', class: 'log-header_icon log-icon_suspended' },
        { tag: 'span', class: 'log-header_text', innerHTML: 'idle in transaction' },
        {
          tag: 'button',
          class: 'log-unsuspend_commit',
          type: 'button',
          disabled: !can_unsuspend,
          onClick: this.unsuspend,
          innerHTML: 'Commit',
        },
      ];
    }

    if (suspended) {
      return [
        { tag: 'span', class: 'log-header_icon log-icon_suspended' },
        { tag: 'span', class: 'log-header_text', innerHTML: 'traffic limit exceeded' },
        {
          tag: 'button',
          class: 'log-unsuspend_more',
          type: 'button',
          disabled: !can_unsuspend,
          onClick: this.unsuspend,
          innerHTML: 'More',
        },
      ];
    }

    if (loading && connecting) {
      return [
        { tag: 'span', class: 'log-header_icon log-icon_ellipsis' },
        { tag: 'span', class: 'log-header_text', innerHTML: 'CONNECTING' },
      ];
    }

    if (loading) {
      return [
        { tag: 'span', class: 'log-header_icon log-icon_ellipsis' },
        { tag: 'span', class: 'log-header_text', innerHTML: 'RUNNING' },
      ];
    }

    if (has_errors) {
      return [
        { tag: 'span', class: 'log-header_icon log-icon_failed' },
        { tag: 'span', class: 'log-header_text', innerHTML: 'FAILED' },
      ];
    }

    return [
      { tag: 'span', class: 'log-header_icon log-icon_ok' },
      { tag: 'span', class: 'log-header_text', innerHTML: 'SUCCEEDED' },
    ];
  },

  /** @param {typeof this.$store.out.messages[number]} m */
  _render_item_content(m) {
    if (m.tag == 'complete') {
      return [
        { tag: 'span', class: 'log-marker log-icon_complete' },
        { tag: 'span', class: 'log-complete', innerText: m.payload },
      ];
    }

    const {
      severity,
      severity_en,
      code,
      message,
      detail,
      hint,
      ...fields
    } = m.payload;
    return [{
      tag: 'details',
      open: m.tag == 'error',
      inner: [
        {
          tag: 'summary',
          inner: [
            { tag: 'span', class: 'log-marker log-icon_ellipsis' },
            {
              tag: 'span',
              class: 'log-prefix',
              'data-severity': severity_en,
              inner: [
                { tag: 'span', class: 'log-severity', innerText: severity },
                code && code != '00000' && { tag: 'span', class: 'log-code', innerText: ' #' + code },
                { tag: 'span', innerHTML: '. ' },
              ],
            },
            { tag: 'span', class: 'log-message', inner: message },
          ]
        }, // summary

        detail && { tag: 'div', class: 'log-detail', innerText: detail },
        hint && { tag: 'div', class: 'log-hint', innerText: hint },
        {
          tag: 'div',
          class: 'log-fields',
          inner: Object.entries(fields).map(([k, v]) => ({
            tag: 'div',
            inner: [
              { tag: 'span', inner: k },
              { tag: 'span', innerHTML: ':&nbsp;' },
              { tag: 'span', inner: v },
            ],
          })),
        },
      ],
    }];
  },
  unsuspend() {
    this.$store.unsuspend();
  },
};

export default {
  methods,
};
