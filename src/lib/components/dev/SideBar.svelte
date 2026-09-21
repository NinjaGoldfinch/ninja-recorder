<!-- The panel list, in registry order, broken into labelled groups. -->

<script lang="ts">
import { navGroups } from "../../dev/panels";

const { activeId }: { activeId: string } = $props();
</script>

<nav class="dev-sidebar">
  {#each navGroups() as group (group.group)}
    <div class="dev-nav-group">{group.group}</div>
    {#each group.panels as panel (panel.id)}
      <a class="dev-nav-link" class:active={panel.id === activeId} href="#/{panel.id}">
        <span class="dev-nav-icon">{panel.icon}</span>{panel.title}
      </a>
    {/each}
  {/each}
</nav>
