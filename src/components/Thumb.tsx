import { useEffect, useState } from 'react';

import { ImageViewer } from './ImageViewer.tsx';
import { stagedThumb, type FileKind } from '../lib/ipc.ts';

/**
 * Thumbnails are fetched per row rather than bundled into the import response —
 * a 200-file batch would otherwise push megabytes of base64 across the IPC bridge
 * before the first row rendered.
 */
export function Thumb({ id, kind }: { id: string; kind: FileKind }) {
  const [src, setSrc] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

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

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        title="Open the page to check it"
        className={`${base} cursor-zoom-in p-0`}
      >
        {src ? (
          <img src={src} alt="" className="h-full w-full object-cover" />
        ) : (
          <span className="text-[10px] font-medium uppercase text-slate-400">
            {kind === 'pdf' ? 'PDF' : kind === 'heic' ? 'HEIC' : '—'}
          </span>
        )}
      </button>

      {open && <ImageViewer ingestId={id} onClose={() => setOpen(false)} />}
    </>
  );
}
