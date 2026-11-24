// Store de l'éditeur distant (M6) : documents ouverts, état modifié/conflit,
// écoute de `edit://event` (sauvegardes automatiques de l'éditeur externe).
import {
  editApi,
  message,
  type EditDocInfo,
  type EditEvent,
  type EditOpenDto,
  type RemoteState,
} from "./api";
import { t } from "./i18n.svelte";
import {
  contentForSave,
  convertEol,
  editorText,
  isDirty,
  targetEol,
  type Eol,
} from "./edit";

export interface EditNotice {
  kind: "ok" | "error";
  text: string;
}

export interface EditDoc {
  info: EditDocInfo;
  /** Référence (dernier état chargé/enregistré), forme « éditeur ». Nul : non éditable en ligne. */
  baseline: string | null;
  /** Texte courant de l'éditeur (forme « éditeur »). */
  current: string | null;
  /** Fin de ligne du document de l'éditeur (détermine le séparateur CodeMirror). */
  baseEol: Eol;
  /** Fin de ligne choisie pour l'enregistrement. */
  eol: Eol;
  /** Incrémenté quand l'éditeur doit reconstruire son état depuis `baseline`. */
  rev: number;
  conflict: RemoteState | null;
  busy: boolean;
  notice: EditNotice | null;
  /** Comparaison en cours : contenu distant (gauche) et local (droite). */
  compare: { remote: string; local: string } | null;
}

export const editor = $state<{
  docs: Record<string, EditDoc>;
  order: string[];
  active: string | null;
  /** Incrémenté pour demander à la vue d'afficher l'éditeur. */
  showSeq: number;
}>({ docs: {}, order: [], active: null, showSeq: 0 });

/**
 * Miroir NON réactif des révisions. Quand `rev` change, le composant CodeEditor est détruit
 * (`{#key}`) et envoie une dernière fois son ancien texte ; or, pendant ce démontage, une
 * lecture réactive de `editor.docs[id].rev` retourne encore l'ANCIENNE valeur (mise à jour
 * différée par le lot de réactivité de Svelte) : le garde-fou doit lire cette copie simple.
 */
const revMirror = new Map<string, number>();

/** Révision à jour d'un document (lecture immédiate, hors réactivité). */
export function currentRev(id: string): number | undefined {
  return revMirror.get(id);
}

function bumpRev(d: EditDoc): void {
  d.rev++;
  revMirror.set(d.info.id, d.rev);
}

let started = false;

export async function initEdit(): Promise<void> {
  if (started) return;
  started = true;
  try {
    for (const info of await editApi.list()) {
      if (editor.docs[info.id]) continue;
      let local: string | null = null;
      try {
        local = info.utf8 ? await editApi.localContent(info.id) : null;
      } catch {
        local = null;
      }
      addDoc({ info, content: local });
    }
  } catch {
    // coffre verrouillé / backend indisponible : la liste se remplira à l'ouverture
  }
  await editApi.onEvent(onEvent).catch(() => {
    // événements indisponibles : l'éditeur intégré reste utilisable
  });
}

function build(dto: EditOpenDto): Omit<EditDoc, "rev"> {
  const baseEol = targetEol(dto.info.line_ending);
  const text = dto.content === null ? null : editorText(dto.content, dto.info.line_ending);
  return {
    info: dto.info,
    baseline: text,
    current: text,
    baseEol,
    eol: baseEol,
    conflict: null,
    busy: false,
    notice: null,
    compare: null,
  };
}

function addDoc(dto: EditOpenDto): string {
  const id = dto.info.id;
  editor.docs[id] = { ...build(dto), rev: 0 };
  revMirror.set(id, 0);
  if (!editor.order.includes(id)) editor.order.push(id);
  editor.active = id;
  return id;
}

/** Ouvre (ou réactive) un fichier distant dans l'éditeur et demande son affichage. */
export async function openRemote(session: string, path: string, external = false): Promise<void> {
  await initEdit();
  const existing = Object.values(editor.docs).find(
    (d) => d.info.session === session && d.info.remote_path === path,
  );
  let id: string;
  if (existing) {
    id = existing.info.id;
    editor.active = id;
  } else {
    id = addDoc(await editApi.open(session, path));
  }
  editor.showSeq++;
  if (external) await openExternal(id);
}

export function activate(id: string): void {
  if (editor.docs[id]) editor.active = id;
}

export function activeDoc(): EditDoc | null {
  return editor.active ? (editor.docs[editor.active] ?? null) : null;
}

export function docCount(): number {
  return editor.order.length;
}

export function docDirty(id: string): boolean {
  const d = editor.docs[id];
  return !!d && !d.info.external && isDirty(d.current, d.baseline, d.eol, d.baseEol);
}

export function setCurrent(id: string, text: string): void {
  const d = editor.docs[id];
  if (d) d.current = text;
}

export function setEol(id: string, eol: Eol): void {
  const d = editor.docs[id];
  if (d) d.eol = eol;
}

function fail(d: EditDoc, e: unknown): void {
  d.notice = { kind: "error", text: message(e) };
}

function applyOpen(d: EditDoc, dto: EditOpenDto): void {
  Object.assign(d, build(dto));
  bumpRev(d);
}

/** Applique un enregistrement réussi d'un document intégré. */
function applySaved(d: EditDoc, info: EditDocInfo, sent: string, eol: Eol): void {
  d.info = info;
  d.conflict = null;
  d.compare = null;
  if (eol !== d.baseEol) {
    // Le séparateur de l'éditeur change : reconstruction depuis le texte envoyé.
    d.baseEol = eol;
    d.baseline = d.current = sent;
    bumpRev(d);
  } else {
    d.baseline = sent;
  }
  d.notice = { kind: "ok", text: t("edit.savedOk") };
}

/** Enregistre le document intégré ; `overwrite` ignore un conflit. `text` : contenu fusionné. */
export async function save(id: string, overwrite = false, text?: string): Promise<void> {
  const d = editor.docs[id];
  if (!d || d.busy || d.current === null) return;
  const sentText = text ?? d.current;
  const eol = d.eol;
  d.busy = true;
  d.notice = null;
  try {
    const out = await editApi.save(id, contentForSave(sentText, eol), overwrite);
    if (out.status === "saved") {
      if (text !== undefined) {
        d.current = text;
        bumpRev(d);
      }
      applySaved(d, out.info, convertEol(sentText, eol), eol);
    } else {
      d.conflict = out.remote;
    }
  } catch (e) {
    fail(d, e);
  } finally {
    d.busy = false;
  }
}

/** Document édité en externe : envoie le contenu actuel du fichier temporaire. */
export async function saveLocal(id: string, overwrite: boolean): Promise<void> {
  const d = editor.docs[id];
  if (!d || d.busy) return;
  d.busy = true;
  d.notice = null;
  try {
    const out = await editApi.saveLocal(id, overwrite);
    if (out.status === "saved") {
      d.info = out.info;
      d.conflict = null;
      d.compare = null;
      d.notice = { kind: "ok", text: t("edit.uploadedOk") };
      await refreshLocal(d);
    } else {
      d.conflict = out.remote;
    }
  } catch (e) {
    fail(d, e);
  } finally {
    d.busy = false;
  }
}

/** Relit le fichier temporaire (éditeur externe) pour l'afficher en lecture seule. */
async function refreshLocal(d: EditDoc): Promise<void> {
  if (!d.info.utf8) return;
  try {
    const local = await editApi.localContent(d.info.id);
    if (local === null) return;
    const text = editorText(local, d.info.line_ending);
    d.baseline = d.current = text;
    bumpRev(d);
  } catch {
    // affichage non rafraîchi : sans gravité
  }
}

/** Écarte les modifications locales : re-télécharge le fichier distant. */
export async function reload(id: string): Promise<void> {
  const d = editor.docs[id];
  if (!d || d.busy) return;
  d.busy = true;
  d.notice = null;
  try {
    applyOpen(d, await editApi.reload(id));
    d.notice = { kind: "ok", text: t("edit.reloaded") };
  } catch (e) {
    fail(d, e);
  } finally {
    d.busy = false;
  }
}

/** Prépare la vue de comparaison (distant à gauche, local éditable à droite). */
export async function startCompare(id: string): Promise<void> {
  const d = editor.docs[id];
  if (!d || d.busy) return;
  d.busy = true;
  d.notice = null;
  try {
    const remote = await editApi.remoteContent(id);
    const local = d.info.external ? await editApi.localContent(id) : d.current;
    if (remote === null || local === null) {
      d.notice = { kind: "error", text: t("edit.compareUnavailable") };
      return;
    }
    d.compare = { remote: convertEol(remote, d.baseEol), local: convertEol(local, d.baseEol) };
  } catch (e) {
    fail(d, e);
  } finally {
    d.busy = false;
  }
}

export function cancelCompare(id: string): void {
  const d = editor.docs[id];
  if (d) d.compare = null;
}

/** Enregistre le résultat fusionné en écrasant le distant. */
export async function saveMerged(id: string, merged: string): Promise<void> {
  const d = editor.docs[id];
  if (!d) return;
  if (d.info.external) {
    // Le fichier temporaire est celui de l'éditeur externe : on envoie la fusion telle quelle.
    d.busy = true;
    try {
      const out = await editApi.save(id, contentForSave(merged, d.eol), true);
      if (out.status === "saved") {
        d.info = out.info;
        d.conflict = null;
        d.compare = null;
        d.baseline = d.current = merged;
        bumpRev(d);
        d.notice = { kind: "ok", text: t("edit.savedOk") };
      } else {
        d.conflict = out.remote;
      }
    } catch (e) {
      fail(d, e);
    } finally {
      d.busy = false;
    }
    return;
  }
  await save(id, true, merged);
}

export async function openExternal(id: string): Promise<void> {
  const d = editor.docs[id];
  if (!d || d.busy) return;
  d.busy = true;
  d.notice = null;
  try {
    await editApi.openExternal(id);
    d.info = { ...d.info, external: true };
    d.notice = { kind: "ok", text: t("edit.externalStarted") };
  } catch (e) {
    fail(d, e);
  } finally {
    d.busy = false;
  }
}

export async function closeDoc(id: string): Promise<void> {
  const d = editor.docs[id];
  if (!d) return;
  try {
    await editApi.close(id);
  } catch (e) {
    fail(d, e);
    return;
  }
  const i = editor.order.indexOf(id);
  editor.order.splice(i, 1);
  delete editor.docs[id];
  revMirror.delete(id);
  if (editor.active === id) editor.active = editor.order[Math.min(i, editor.order.length - 1)] ?? null;
}

function onEvent(ev: EditEvent): void {
  const d = editor.docs[ev.id];
  if (!d) return;
  if (ev.type === "saved") {
    d.info = ev.info;
    d.conflict = null;
    d.notice = { kind: "ok", text: t("edit.uploadedOk") };
    void refreshLocal(d);
  } else if (ev.type === "conflict") {
    d.conflict = ev.remote;
  } else {
    d.notice = { kind: "error", text: message(ev.message) };
  }
}
