<!-- Every panel's heading and its one-line explanation of what it is for. -->

<script lang="ts">
const { title, description }: { title: string; description: string } = $props();
</script>

<div class="panel-head">
  <h1>{title}</h1>
  <p>{description}</p>
</div>
