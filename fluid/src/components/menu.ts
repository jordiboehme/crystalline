/**
 * The classes every dropdown in this app is drawn with.
 *
 * One definition rather than one per menu: the theme menu, the account menu
 * and the domain switcher are the same control in three places, and a reader
 * who has learned what one looks like should not have to learn the others.
 */

/** The surface a menu opens onto. */
export const MENU_CLASSES =
  "z-50 min-w-48 rounded border border-slate-200 bg-white p-1 text-sm shadow-lg dark:border-slate-700 dark:bg-slate-900";

/** One row inside it. */
export const ITEM_CLASSES =
  "flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 outline-none select-none data-[highlighted]:bg-slate-100 dark:data-[highlighted]:bg-slate-800";
