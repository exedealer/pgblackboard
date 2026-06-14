const methods = {
  _render() {
    const { pending, error } = this.$store.auth;

    return {
      tag: 'form',
      class: 'auth',
      onSubmit: this.on_submit,
      inner: [
        {
          tag: 'input',
          class: 'auth-user',
          type: 'text',
          name: 'user',
          required: true,
          placeholder: 'user',
          autofocus: true,
          autocomplete: 'username',
          spellcheck: false,
        },

        {
          tag: 'input',
          class: 'auth-password',
          type: 'password',
          name: 'password',
          placeholder: 'password',
          autocomplete: 'current-password',
          'aria-describedby': 'auth-error',

          ... Boolean(error) && {
            'aria-invalid': 'true',
          },
        },

        {
          tag: 'button',
          class: 'auth-submit',
          disabled: pending,
          inner: 'Log in',
        },

        {
          tag: 'div',
          id: 'auth-error',
          class: 'auth-error',
          role: 'alert',
          inner: error,
        },
      ],
    };
  },
  /** @param {SubmitEvent} e */
  on_submit(e) {
    e.preventDefault();
    const form = new FormData(e.target);
    this.$store.auth_submit(form.get('user'), form.get('password'));
  },
};

export default { methods };
