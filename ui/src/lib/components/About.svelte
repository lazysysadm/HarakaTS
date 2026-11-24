<script lang="ts">
  import { t } from "../i18n.svelte";
  import { onMount } from "svelte";
  import { aboutApi, message, type AboutInfo } from "../api";

  interface Props {
    onerror: (msg: string) => void;
  }
  let { onerror }: Props = $props();

  let about = $state<AboutInfo | null>(null);
  let thirdParty = $state<string | null>(null);
  let thirdPartyOpen = $state(false);

  onMount(() => {
    aboutApi
      .info()
      .then((a) => (about = a))
      .catch((e) => onerror(message(e)));
  });

  async function toggleThirdParty() {
    if (thirdParty === null) {
      try {
        thirdParty = await aboutApi.thirdParty();
      } catch (e) {
        onerror(message(e));
        return;
      }
    }
    thirdPartyOpen = !thirdPartyOpen;
  }
</script>

{#if about}
  <div class="about">
    <h3>{about.name} <span class="muted">v{about.version} · {about.git_hash}</span></h3>
    <p>{about.description}</p>

    <table class="about-table">
      <tbody>
        <tr><th>{t("about.version")}</th><td>{about.version} <code>({about.git_hash})</code></td></tr>
        <tr><th>{t("about.author")}</th><td>{about.author}</td></tr>
        <tr><th>{t("about.license")}</th><td>{about.license_name}</td></tr>
        <tr><th>{t("about.platform")}</th><td>{about.os} / {about.arch}</td></tr>
        <tr><th>{t("about.interface")}</th><td>{t("about.uiStack", { version: about.tauri_version })}</td></tr>
        <tr><th>SSH / SFTP</th><td>russh · russh-sftp (Rust)</td></tr>
        <tr><th>{t("about.crypto")}</th><td>Argon2id · XChaCha20-Poly1305</td></tr>
        <tr><th>{t("about.data")}</th><td><code>{about.data_dir}</code></td></tr>
      </tbody>
    </table>

    <h4>{t("about.licenseTitle", { name: about.license_name })}</h4>
    <pre class="license">{about.license_text}</pre>

    <button type="button" class="secondary" onclick={() => void toggleThirdParty()}>
      {t("about.thirdParty")}
    </button>
    {#if thirdPartyOpen && thirdParty}
      <pre class="license third-party">{thirdParty}</pre>
    {/if}
  </div>
{:else}
  <p class="muted">{t("common.loading")}</p>
{/if}
