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
   * Two front sources: the game's uploaded front bundle, served same-origin by
   * the server at /api/ws/<slug>/g/<game>/front/ (default when one exists — no
   * localhost or deployed URL needed at all), or a custom URL (a dev server or
   * a deployed build) for front iteration.
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
  /** Which front the test view iframes: the uploaded bundle or a custom URL. */
  let source = $state<'hosted' | 'custom'>('custom');
  /** null = probing; then whether the game has an uploaded bundle. */
  let hostedAvailable = $state<boolean | null>(null);

  let hostedPath = $derived(
    `/api/ws/${encodeURIComponent(slug)}/g/${encodeURIComponent(game)}/front/`
  );
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

  // Hydrate the remembered URL, probe for an uploaded bundle (HEAD on the
  // front route — axum answers HEAD for GET routes), and default the source to
  // the hosted bundle when one exists.
  $effect(() => {
    if (!open) return;
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved) url = saved;
    } catch {
      // localStorage may be unavailable; the dialog still works without it.
    }
    hostedAvailable = null;
    void fetch(hostedPath, { method: 'HEAD', credentials: 'same-origin' })
      .then((r) => {
        hostedAvailable = r.ok;
        if (r.ok) source = 'hosted';
      })
      .catch(() => {
        hostedAvailable = false;
      });
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
    let front: string;
    if (source === 'hosted') {
      front = `${location.origin}${hostedPath}`;
    } else {
      if (!parsed.ok) return;
      front = url.trim();
      try {
        localStorage.setItem(storageKey, front);
      } catch {
        // Non-fatal — just means we won't remember it next time.
      }
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
        <fieldset class="mb-4 space-y-2">
          <legend class="mb-1 text-sm font-medium">Game front</legend>
          <label
            class="flex items-start gap-2 text-sm {hostedAvailable === false
              ? 'cursor-not-allowed opacity-50'
              : 'cursor-pointer'}"
          >
            <input
              type="radio"
              name="front-source"
              value="hosted"
              bind:group={source}
              disabled={hostedAvailable === false}
              class="mt-0.5"
            />
            <span>
              Uploaded front bundle
              {#if hostedAvailable === null}
                <span class="text-faint">(checking…)</span>
              {:else if hostedAvailable === false}
                <span class="text-faint">— none uploaded yet (push one from the Revisions tab)</span>
              {:else}
                <span class="text-faint">— served by this server, no URL needed</span>
              {/if}
            </span>
          </label>
          <label class="flex items-start gap-2 text-sm cursor-pointer">
            <input type="radio" name="front-source" value="custom" bind:group={source} class="mt-0.5" />
            <span>Custom URL <span class="text-faint">— your dev server or a deployed build</span></span>
          </label>
        </fieldset>

        {#if source === 'custom'}
          <Input
            id={INPUT_ID}
            label="Game front URL"
            bind:value={url}
            mono
            placeholder="https://your-game-front.example.com"
            error={parsed.error || undefined}
            hint="The front to load in the iframe."
          />

          {#if parsed.mixed}
            <p class="mt-3 rounded-md border border-warn/30 bg-warn/10 px-3 py-2 text-xs text-warn">
              This is an <span class="font-mono-tab">http://</span> URL. The dashboard is served over
              https, so the browser will block it as mixed content. Use an
              <span class="font-mono-tab">https://</span> front.
            </p>
          {/if}
        {/if}

        <div class="mt-5 flex justify-end gap-2">
          <Button type="button" variant="ghost" onclick={close}>Cancel</Button>
          <Button
            type="submit"
            disabled={source === 'hosted' ? hostedAvailable !== true : !parsed.ok}
          >
            Open test view ↗
          </Button>
        </div>
      </form>
    </div>
  </div>
{/if}
