<script lang="ts">
  /**
   * Time — renders a relative age ("3h ago") with the absolute local date+time
   * as its `title` tooltip, wrapped in a real `<time datetime>` element. One
   * component so every timestamp in the app reads and behaves the same.
   */
  import { relativeAge, formatAbsolute } from '$lib/format';

  type Props = { iso: string | null | undefined; class?: string };
  let { iso, class: klass = '' }: Props = $props();
</script>

<time datetime={iso || undefined} title={formatAbsolute(iso)} class={klass}>{relativeAge(iso)}</time>
