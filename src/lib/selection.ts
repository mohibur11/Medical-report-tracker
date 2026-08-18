/**
 * Multi-select with shift-range, kept out of the component so it can be tested.
 *
 * A backlog import is 200-1000 rows. Selecting them one click at a time is the
 * same trap as answering "which patient?" 200 times, so shift-range is not a
 * nicety here — it is the difference between the bulk bar being usable and not.
 */

export interface Selection {
  readonly ids: ReadonlySet<string>;
  /** Where a shift-range starts. The last row clicked without shift. */
  readonly anchor: string | null;
}

export const EMPTY: Selection = { ids: new Set(), anchor: null };

/**
 * Apply one click on `id`.
 *
 * Shift extends from the anchor and only ever adds, which is what a spreadsheet
 * does — a shift-click that silently deselected rows would be a quiet way to
 * lose typed corrections.
 */
export function click(
  state: Selection,
  order: readonly string[],
  id: string,
  shift: boolean,
): Selection {
  if (shift && state.anchor !== null && state.anchor !== id) {
    const from = order.indexOf(state.anchor);
    const to = order.indexOf(id);
    if (from !== -1 && to !== -1) {
      const [lo, hi] = from < to ? [from, to] : [to, from];
      const ids = new Set(state.ids);
      for (const between of order.slice(lo, hi + 1)) ids.add(between);
      // The anchor stays put, so dragging the range back and forth works.
      return { ids, anchor: state.anchor };
    }
  }

  const ids = new Set(state.ids);
  if (ids.has(id)) ids.delete(id);
  else ids.add(id);
  return { ids, anchor: id };
}

/** Select every row, or none if every row is already selected. */
export function toggleAll(state: Selection, order: readonly string[]): Selection {
  return allSelected(state, order) ? EMPTY : { ids: new Set(order), anchor: null };
}

export function allSelected(state: Selection, order: readonly string[]): boolean {
  return order.length > 0 && order.every((id) => state.ids.has(id));
}

/**
 * Drop ids that are no longer on screen.
 *
 * Filing removes rows, and a selection holding ids of committed documents would
 * make the count lie and could re-target a later bulk action at nothing.
 */
export function prune(state: Selection, order: readonly string[]): Selection {
  const present = new Set(order);
  const ids = new Set([...state.ids].filter((id) => present.has(id)));
  if (ids.size === state.ids.size) return state;
  return { ids, anchor: state.anchor !== null && present.has(state.anchor) ? state.anchor : null };
}

/** Selected ids in screen order, so bulk filing follows what the user sees. */
export function ordered(state: Selection, order: readonly string[]): string[] {
  return order.filter((id) => state.ids.has(id));
}
