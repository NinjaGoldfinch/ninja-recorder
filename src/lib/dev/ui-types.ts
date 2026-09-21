/**
 * Shapes the portal's presentational components share.
 *
 * In a module rather than in a `<script>` block: a component's instance script
 * cannot export a type, and `<script module>` for two interfaces is more
 * ceremony than a file.
 */

export type PillTone = "" | "ok" | "warn" | "danger";

export interface ConfirmOptions {
  title: string;
  body: string;
  confirmLabel?: string;
  /**
   * When set, the confirm button waits for this exact text.
   *
   * Reserved for operations that cannot be undone: it is the difference
   * between a click you meant and a click you were already making.
   */
  typeToConfirm?: string;
}
