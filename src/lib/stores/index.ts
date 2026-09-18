/**
 * Application state as Svelte 5 runes — WS4 task 4.3.
 *
 * One shape, fed two ways: the daemon's snapshot sets it on connect, and the
 * event stream updates it after that. Nothing polls, and nothing derives
 * state from a command's return value — a command that changed something
 * produces an event, and the event is what moves the store, so a change made
 * by the tray or by another window lands the same way as one made here
 * (implementation plan §4.2, §4.6).
 */
export {};
