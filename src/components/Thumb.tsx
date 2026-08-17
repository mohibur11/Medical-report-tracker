import { useEffect, useState } from 'react';

import { stagedThumb, type FileKind } from '../lib/ipc.ts';

/**
 * Thumbnails are fetched per row rather than bundled into the import response —
 * a 200-file batch would otherwise push megabytes of base64 across the IPC bridge
 * before the first row rendered.
 */
export function Thumb({ id, kind }: { id: string; kind: FileKind }) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    stagedThumb(id).then(
      (s) => alive && setSrc(s),
      () => {},
    );
    return () => {
      alive = false;
    };
  }, [id]);

  const base =
    'flex h-16 w-12 shrink-0 items-center justify-center overflow-hidden rounded border ' +
    'border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-800';

  if (src) {
    return (
      <div className={base}>
        <img src={src} alt="" className="h-full w-full object-cover" />
      </div>
    );
  }

  // PDFs have no raster thumbnail until pdf.js renders page 1 (Phase 1, later step).
  return (
    <div className={`${base} text-[10px] font-medium uppercase text-slate-400`}>
      {kind === 'pdf' ? 'PDF' : kind === 'heic' ? 'HEIC' : '—'}
    </div>
  );
}
