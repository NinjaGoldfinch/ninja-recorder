<!--
  Version, what the client is doing, and the updater.

  Three of these lines are pushed by `status.ts`'s poll rather than read when
  the view opens, which is why they come from their own store: the poll runs
  whether or not anyone is looking at this panel.
-->

<script lang="ts">
import { about } from "../../stores/about.svelte";
import { settings } from "../../stores/settings.svelte";
import Update from "./Update.svelte";
</script>

<section class="settings-group">
  <h3>About</h3>
  <dl class="about-list">
    <div><dt>Version</dt><dd>{settings.version}</dd></div>
    <div><dt>League Client</dt><dd>{about.lcu}</dd></div>
    <div><dt>Game state</dt><dd>{about.gameState}</dd></div>
    <div>
      <dt>Last recording</dt>
      <dd class="mono">{about.lastFinalized}</dd>
    </div>
    <!-- Static text, and deliberately not a link: the file is in the install
         folder, which the UI has no command to open, and the notices are
         generated rather than written here (docs/licensing.md §5). -->
    <div>
      <dt>Third-party notices</dt>
      <dd><span class="mono">THIRD_PARTY_NOTICES.txt</span>, in the install folder</dd>
    </div>
    <Update />
  </dl>
</section>
