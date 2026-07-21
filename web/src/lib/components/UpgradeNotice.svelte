<script lang="ts">
  /**
   * UpgradeNotice — the inline surface for a quota / expired-trial write refusal
   * (`upgrade_required`, `storage_quota_exceeded`). Shows the friendly message
   * plus an "Upgrade →" link to the workspace billing page, in place of a bare
   * error box. Kept small and self-contained so panels adopt it with one line.
   */
  type Props = { slug: string; message: string; class?: string };
  let { slug, message, class: klass = '' }: Props = $props();
</script>

<div
  class="flex flex-col gap-1.5 rounded-md border border-warn/40 bg-warn/10 px-3 py-2.5 text-sm text-warn sm:flex-row sm:items-center sm:justify-between sm:gap-3 {klass}"
>
  <span>{message}</span>
  <a
    href={`/w/${slug}/billing`}
    class="shrink-0 font-medium whitespace-nowrap underline-offset-4 hover:underline"
  >
    Upgrade →
  </a>
</div>
