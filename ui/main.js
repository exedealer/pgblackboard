import monaco_worker from './_vendor/monaco_worker.js';
import monaco_json_worker from './_vendor/monaco_json_worker.js';
import { editor } from './_vendor/monaco.js';
import { createApp, reactive, watch, h as create_vnode, computed } from './_vendor/vue.js';
import { Store } from './store/store.js';
import xRoot from './app/app.js';

globalThis.MonacoEnvironment = {
  getWorker(_module_id, label) {
    switch (label) {
      case 'json': return monaco_json_worker();
      default: return monaco_worker();
    }
  },
};

editor.defineTheme('pgbb-dark', {
  base: 'vs-dark',
  inherit: true,
  colors: {
    'editorLineNumber.foreground': '#5c5c5c',
  },
  rules: [
    { token: 'comment', foreground: '888888' },
    { token: 'string.sql', foreground: 'CE9178' },
  ],
});

editor.defineTheme('pgbb-light', {
  base: 'vs',
  inherit: true,
  colors: {},
  rules: [
    { token: 'comment', foreground: '888888' },
    { token: 'string.sql', foreground: 'A31515' },
  ],
});

const store = reactive(new Store());
globalThis.debug_store = store;

const m = matchMedia('(prefers-color-scheme: dark)');
store.set_dark_sys(m.matches);
m.addEventListener('change', _ => store.set_dark_sys(m.matches));

watch(
  _ => store.is_dark(),
  is_dark => editor.setTheme(is_dark ? 'pgbb-dark' : 'pgbb-light'),
  { immediate: true },
);

watch(
  _ => store.is_dark(null),
  is_dark => document.documentElement.setAttribute('data-theme', is_dark == null ? 'sys' : is_dark ? 'dark' : 'light'),
  { immediate: true },
);

const app = createApp(create_vnode(xRoot));
app.config.globalProperties.$store = store;
app.config.globalProperties.$cached = function (fn) {
  const c = fn._vue_computed ||= computed(fn);
  return c.value;
};
app.use(pojo_vdom_plugin);
app.use(broadcast_plugin);
app.mount('body');

function pojo_vdom_plugin(app) {
  app.mixin({
    render() {
      return transform_vdom(this._render());
    },

    // TODO not related to pojo vdom
    mounted() {
      this._mounted?.();
    },
    unmounted() {
      this._unmounted?.();
    },
  });

  function transform_vdom(def) {
    if (typeof def == 'function') { // default slot
      return (...args) => transform_vdom(def(...args));
    }
    if (Array.isArray(def)) {
      for (let i = 0; i < def.length; i++) {
        def[i] = transform_vdom(def[i]);
      }
    };
    const { tag, inner } = def || 0;
    if (tag) {
      def.tag = undefined;
      def.inner = undefined;
      def = create_vnode(tag, def, transform_vdom(inner));
    }
    return def;
  }
}

function broadcast_plugin(app) {
  const et = new EventTarget();

  app.mixin({
    mounted() {
      const broadcast_listeners = this._get_broadcast_listeners?.();
      if (!broadcast_listeners) return;
      this._broadcast_listeners = broadcast_listeners;
      for (const [event, listener] of Object.entries(broadcast_listeners)) {
        et.addEventListener(event, listener);
      }
    },
    unmounted() {
      if (!this._broadcast_listeners) return;
      for (const [event, listener] of Object.entries(broadcast_listeners)) {
        et.removeEventListener(event, listener);
      }
    },
    methods: {
      $broadcast(event, details) {
        et.dispatchEvent(new CustomEvent(event, { details }));
      },
    },
  });
}

// https://github.com/microsoft/monaco-editor/issues/4379
// https://bugzilla.mozilla.org/show_bug.cgi?id=1556240
// fix click event on monaco and grip component
function fix_firefox_click(doc) {
  doc.addEventListener('pointerup', on_pointerup, true);
  doc.addEventListener('click', on_click, true);

  let last_pointerup_target;

  function on_pointerup(event) {
    last_pointerup_target = event.target;
  }

  function on_click(event) {
    if (!event.pointerType) return; // allow syntetic clicks by kEnter
    if (last_pointerup_target == event.target) return;
    event.stopPropagation();
    event.preventDefault();
  }
}

fix_firefox_click(document);
