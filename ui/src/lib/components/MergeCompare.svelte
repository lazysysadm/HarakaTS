<script lang="ts">
  import { onMount } from "svelte";
  import { Compartment, EditorState } from "@codemirror/state";
  import { EditorView, keymap, lineNumbers } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
  import { MergeView } from "@codemirror/merge";
  import { t } from "../i18n.svelte";
  import { uiPrefs } from "../state.svelte";
  import { appTheme, cmPhrases, isDarkTheme, loadLanguage } from "../cm-theme";

  interface Props {
    /** Contenu distant (gauche, lecture seule). */
    remote: string;
    /** Contenu local (droite, modifiable). */
    local: string;
    name: string;
    sep: string;
    busy: boolean;
    onsave: (merged: string) => void;
    oncancel: () => void;
  }
  let { remote, local, name, sep, busy, onsave, oncancel }: Props = $props();

  let host: HTMLDivElement;
  let mv: MergeView | null = null;
  const themeA = new Compartment();
  const themeB = new Compartment();
  const langA = new Compartment();
  const langB = new Compartment();

  function base(readOnly: boolean, theme: Compartment, lang: Compartment) {
    return [
      EditorState.lineSeparator.of(sep),
      cmPhrases(),
      lineNumbers(),
      history(),
      keymap.of([...historyKeymap, ...defaultKeymap]),
      theme.of(appTheme(true)),
      lang.of([]),
      EditorState.readOnly.of(readOnly),
      EditorView.editable.of(!readOnly),
    ];
  }

  onMount(() => {
    let alive = true;
    const view = new MergeView({
      a: { doc: remote, extensions: base(true, themeA, langA) },
      b: { doc: local, extensions: base(false, themeB, langB) },
      parent: host,
      gutter: true,
      highlightChanges: true,
      revertControls: "a-to-b",
      collapseUnchanged: { margin: 3, minSize: 6 },
    });
    mv = view;
    const dark = appTheme(isDarkTheme());
    view.a.dispatch({ effects: themeA.reconfigure(dark) });
    view.b.dispatch({ effects: themeB.reconfigure(dark) });
    void loadLanguage(name).then((l) => {
      if (!alive || !l) return;
      view.a.dispatch({ effects: langA.reconfigure(l.support) });
      view.b.dispatch({ effects: langB.reconfigure(l.support) });
    });
    return () => {
      alive = false;
      view.destroy();
      mv = null;
    };
  });

  $effect(() => {
    void uiPrefs.theme;
    const h = setTimeout(() => {
      const th = appTheme(isDarkTheme());
      mv?.a.dispatch({ effects: themeA.reconfigure(th) });
      mv?.b.dispatch({ effects: themeB.reconfigure(th) });
    }, 0);
    return () => clearTimeout(h);
  });
</script>

<div class="cmp">
  <div class="cmp-bar">
    <span class="muted small">{t("edit.compareHint")}</span>
    <span class="spacer"></span>
    <button type="button" class="tb primary" disabled={busy} onclick={() => mv && onsave(mv.b.state.sliceDoc())}>{t("edit.saveMerged")}</button>
    <button type="button" class="tb" disabled={busy} onclick={oncancel}>{t("edit.cancelCompare")}</button>
  </div>
  <div class="cmp-labels">
    <span>{t("edit.remoteSide")}</span>
    <span>{t("edit.localSide")}</span>
  </div>
  <div class="cm-host" bind:this={host}></div>
</div>
