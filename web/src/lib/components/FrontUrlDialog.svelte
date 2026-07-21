<script lang="ts">
  /**
   * FrontUrlDialog — collects the game-front URL, then opens the embedded
   * multi-resolution test view against the cloud LGS in a new tab.
   *
   * The test view is the SAME page the desktop app embeds; on the cloud it is
   * served by the tenant router at
   *   /api/ws/<slug>/g/<game>/r/<number>/test/
   * same-origin with this dashboard (so the session cookie authorizes it). It
   * only needs two query params — `gameSlug` (which math to resolve) and
   * `gameUrl` (the front bundle to iframe). `gameSlug` is the game itself; the
   * front URL is supplied here and remembered per game.
   *
   * M5 will host front bundles same-origin (they ship with share links), at
   * which point this dialog can default to the hosted bundle. Until then the
   * tester brings their own front URL (a dev server or a deployed build).
   */
  import Button from '$lib/components/Button.svelte';
  import Input from '$lib/components/Input.svelte';

  type Props = {
    slug: string;
    game: string;
    /** Revision number the test view pins to (head on the game page; this rev on the detail page). */
    number: number;
    open?: boolean;
    onclose?: () => void;
  };

  let { slug, game, number, open = $bindable(false), onclose }: Props = $props();

  const INPUT_ID = 'fronturl-input';
  let url = $state('');

  let storageKey = $derived(`stake-cloud:testview-front:${slug}:${game}`);
  let pageIsHttps = $derived(typeof location !== 'undefined' && location.protocol === 'https:');

  // Validation derived from the current input: `ok` gates the submit button;
  // `mixed` flags an http:// front that an https:// dashboard will block.
  let parsed = $derived.by(() => {
    const raw = url.trim();
    if (!raw) return { ok: false, mixed: false, error: '' };
    let u: URL;
    try {
      u = new URL(raw);
    } catch {
      return { ok: false, mixed: false, error: 'Enter a full URL, including https://' };
    }
    if (u.protocol !== 'http:' && u.protocol !== 'https:') {
      return { ok: false, mixed: false, error: 'Only http(s) URLs are supported.' };
    }
    return { ok: true, mixed: u.protocol === 'http:' && pageIsHttps, error: '' };
  });

  // Hydrate the remembered URL and focus the input each time the dialog opens.
  $effect(() => {
    if (!open) return;
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved) url = saved;
    } catch {
      // localStorage may be unavailable; the dialog still works without it.
    }
    requestAnimationFrame(() => document.getElementById(INPUT_ID)?.focus());
  });

  function close() {
    open = false;
    onclose?.();
  }

  function onWindowKeydown(e: KeyboardEvent) {
    if (open && e.key === 'Escape') close();
  }

  function submit() {
    if (!parsed.ok) return;
    const front = url.trim();
    try {
      localStorage.setItem(storageKey, front);
    } catch {
      // Non-fatal — just means we won't remember it next time.
    }
    const qs = new URLSearchParams({ gameSlug: game, gameUrl: front });
    const target = `/api/ws/${encodeURIComponent(slug)}/g/${encodeURIComponent(game)}/r/${number}/test/?${qs.toString()}`;
    window.open(target, '_blank', 'noopener,noreferrer');
    close();
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#if open}
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
    <!-- Backdrop (a real <button> so the click-to-close is keyboard-accessible). -->
    <button type="button" class="absolute inset-0 bg-black/60" aria-label="Close" onclick={close}
    ></button>

    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="fronturl-title"
      class="fade-in relative z-10 w-full max-w-md rounded-lg border border-border bg-surface p-6 shadow-xl"
    >
      <div class="mb-1 flex items-center justify-between gap-3">
        <h2 id="fronturl-title" class="text-base font-semibold">Open test view</h2>
        <span class="font-mono-tab text-xs text-faint">{game} · rev {number}</span>
      </div>
      <p class="mb-4 text-sm text-muted">
        Runs the multi-resolution test view against this revision on the cloud server, in a new tab.
      </p>

      <form
        onsubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <Input
          id={INPUT_ID}
          label="Game front URL"
          bind:value={url}
          mono
          placeholder="https://your-game-front.example.com"
          error={parsed.error || undefined}
          hint="The front bundle to load in the iframe — your dev server or a deployed build."
        />

        {#if parsed.mixed}
          <p class="mt-3 rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn">
            This is an <span class="font-mono-tab">http://</span> URL. The dashboard is served over
            https, so the browser will block it as mixed content. Use an
            <span class="font-mono-tab">https://</span> front.
          </p>
        {/if}

        <p class="mt-3 rounded-md border border-info/30 bg-info/10 px-3 py-2 text-xs text-info">
          Hosted front bundles arrive with share links (M5) — this will default to the hosted bundle
          once that lands.
        </p>

        <div class="mt-5 flex justify-end gap-2">
          <Button type="button" variant="ghost" onclick={close}>Cancel</Button>
          <Button type="submit" disabled={!parsed.ok}>Open test view ↗</Button>
        </div>
      </form>
    </div>
  </div>
{/if}
