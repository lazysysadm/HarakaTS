<script module lang="ts">
  import type { EditorState as CmState } from "@codemirror/state";

  // États conservés par document : l'historique d'annulation survit au changement d'onglet.
  const stateCache = new Map<string, { rev: number; state: CmState }>();

  export function dropEditorState(id: string): void {
    stateCache.delete(id);
  }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import { Compartment, EditorState } from "@codemirror/state";
  import {
    EditorView,
    drawSelection,
    highlightActiveLine,
    highlightActiveLineGutter,
    highlightSpecialChars,
    keymap,
    lineNumbers,
  } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { bracketMatching, indentOnInput } from "@codemirror/language";
  import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
  import { uiPrefs } from "../state.svelte";
  import { appTheme, cmPhrases, isDarkTheme, loadLanguage } from "../cm-theme";

  interface Props {
    id: string;
    rev: number;
    /** Texte initial (forme « éditeur », séparateur `sep`). */
    text: string;
    name: string;
    sep: string;
    readonly: boolean;
    /** `rev` : révision du document au montage (pour ignorer un envoi tardif après remplacement). */
    onchange: (text: string, rev: number) => void;
    oncursor: (line: number, col: number) => void;
    onlang: (name: string | null) => void;
    onsave: () => void;
  }
  let { id, rev, text, name, sep, readonly, onchange, oncursor, onlang, onsave }: Props = $props();

  // Révision AU MONTAGE : le composant est remonté à chaque changement de `rev` (via {#key}) et,
  // pendant ce démontage, la valeur courante de la prop ne doit pas servir (cf. `currentRev`).
  // svelte-ignore state_referenced_locally
  const mountRev = rev;

  let host: HTMLDivElement;
  let view: EditorView | null = null;
  const themeC = new Compartment();
  const langC = new Compartment();
  const roC = new Compartment();
  let timer: ReturnType<typeof setTimeout> | undefined;

  /** Texte courant, immédiatement (avant un enregistrement). */
  export function getText(): string {
    return view ? view.state.sliceDoc() : text;
  }

  function flush() {
    clearTimeout(timer);
    timer = undefined;
    if (view) onchange(view.state.sliceDoc(), mountRev);
  }

  function readOnlyExt(ro: boolean) {
    return [EditorState.readOnly.of(ro), EditorView.editable.of(!ro)];
  }

  function cursorInfo(state: EditorState) {
    const head = state.selection.main.head;
    const line = state.doc.lineAt(head);
    oncursor(line.number, head - line.from + 1);
  }

  onMount(() => {
    let alive = true;
    const cached = stateCache.get(id);
    const state =
      cached && cached.rev === mountRev
        ? cached.state
        : EditorState.create({
            doc: text,
            extensions: [
              EditorState.lineSeparator.of(sep),
              cmPhrases(),
              lineNumbers(),
              highlightActiveLineGutter(),
              highlightSpecialChars(),
              history(),
              drawSelection(),
              indentOnInput(),
              bracketMatching(),
              highlightActiveLine(),
              highlightSelectionMatches(),
              search({ top: true }),
              keymap.of([
                {
                  key: "Mod-s",
                  preventDefault: true,
                  run: () => {
                    flush();
                    onsave();
                    return true;
                  },
                },
                indentWithTab,
                ...searchKeymap,
                ...historyKeymap,
                ...defaultKeymap,
              ]),
              themeC.of(appTheme(true)),
              langC.of([]),
              roC.of([]),
            ],
          });
    view = new EditorView({
      state,
      parent: host,
      dispatch: (tr, v) => {
        v.update([tr]);
        if (tr.docChanged) {
          clearTimeout(timer);
          timer = setTimeout(flush, 120);
        }
        if (tr.docChanged || tr.selection) cursorInfo(v.state);
      },
    });
    view.dispatch({
      effects: [themeC.reconfigure(appTheme(isDarkTheme())), roC.reconfigure(readOnlyExt(readonly))],
    });
    cursorInfo(view.state);
    if (!readonly) view.focus();
    void loadLanguage(name).then((l) => {
      if (!alive || !view) return;
      onlang(l?.name ?? null);
      if (l) view.dispatch({ effects: langC.reconfigure(l.support) });
    });
    return () => {
      alive = false;
      if (view) {
        flush();
        stateCache.set(id, { rev: mountRev, state: view.state });
        view.destroy();
        view = null;
      }
    };
  });

  $effect(() => {
    const ro = readonly;
    view?.dispatch({ effects: roC.reconfigure(readOnlyExt(ro)) });
  });

  // Thème de l'application (ou du système) modifié : recalcule clair/sombre.
  let sysTick = $state(0);
  onMount(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    const on = () => sysTick++;
    mq?.addEventListener("change", on);
    return () => mq?.removeEventListener("change", on);
  });
  $effect(() => {
    void uiPrefs.theme;
    void sysTick;
    const h = setTimeout(() => view?.dispatch({ effects: themeC.reconfigure(appTheme(isDarkTheme())) }), 0);
    return () => clearTimeout(h);
  });
</script>

<div class="cm-host" bind:this={host}></div>
