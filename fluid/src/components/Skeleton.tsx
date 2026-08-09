/** The loading shape every screen shows: rows where content will land. */
export function Skeleton({
  label,
  rows = 3,
}: {
  label: string;
  rows?: number;
}) {
  return (
    <div role="status" aria-busy="true" aria-label={label}>
      <div aria-hidden="true" className="flex animate-pulse flex-col gap-2">
        {Array.from({ length: rows }, (_, row) => (
          <div
            key={row}
            className="h-6 rounded bg-slate-100 dark:bg-slate-800"
          />
        ))}
      </div>
    </div>
  );
}
