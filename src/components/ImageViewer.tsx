import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

import { stagedPreview } from '../lib/ipc.ts';

/**
 * The page, big enough to read, on both platforms.
 *
 * Confirming a date means looking at where it was read from. A 320-pixel
 * thumbnail cannot answer that question, so tapping one opens the page itself
 * with zoom: wheel or the buttons on a desktop, pinch and drag on a phone.
 *
 * Zoom is done with a transform rather than by loading anything larger. The
 * picture is already in memory, and re-fetching at a higher resolution would put
 * a wait exactly where somebody is trying to check something quickly.
 *
 * Three separate ways out, because a full-screen overlay somebody cannot dismiss
 * is worse than no preview at all: the close button, the phone's back gesture,
 * and Escape. The overlay is also rendered into `document.body` rather than where
 * it is written, so nothing it happens to sit inside can clip it.
 */
export function ImageViewer({ ingestId, onClose }: { ingestId: string; onClose: () => void }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });

  /** Pointers currently down, for pinch — a phone has no wheel. */
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinchStart = useRef<{ distance: number; scale: number } | null>(null);
  const dragStart = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);

  useEffect(() => {
    let alive = true;
    stagedPreview(ingestId).then(
      (data) => {
        if (!alive) return;
        if (data) setSrc(data);
        else setFailed(true);
      },
      () => alive && setFailed(true),
    );
    return () => {
      alive = false;
    };
  }, [ingestId]);

  // Escape on a desktop, and the phone's back gesture — which otherwise leaves
  // the app entirely, which is a startling thing to happen when somebody meant
  // to shut a picture.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    const onPop = () => onClose();

    window.history.pushState({ viewer: true }, '');
    window.addEventListener('keydown', onKey);
    window.addEventListener('popstate', onPop);

    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('popstate', onPop);
      // Take our own history entry back out, unless the back gesture is what
      // removed it — going back twice would leave the screen behind as well.
      if (window.history.state?.viewer) window.history.back();
    };
  }, [onClose]);

  const clamp = (value: number) => Math.min(6, Math.max(1, value));

  function onPointerDown(e: React.PointerEvent) {
    (e.target as Element).setPointerCapture?.(e.pointerId);
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });

    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      pinchStart.current = {
        distance: Math.hypot(a!.x - b!.x, a!.y - b!.y),
        scale,
      };
      dragStart.current = null;
    } else if (pointers.current.size === 1) {
      dragStart.current = { x: e.clientX, y: e.clientY, ox: offset.x, oy: offset.y };
    }
  }

  function onPointerMove(e: React.PointerEvent) {
    if (!pointers.current.has(e.pointerId)) return;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });

    if (pointers.current.size === 2 && pinchStart.current) {
      const [a, b] = [...pointers.current.values()];
      const distance = Math.hypot(a!.x - b!.x, a!.y - b!.y);
      setScale(clamp((distance / pinchStart.current.distance) * pinchStart.current.scale));
      return;
    }

    // Panning only matters once there is more picture than screen.
    if (dragStart.current && scale > 1) {
      setOffset({
        x: dragStart.current.ox + (e.clientX - dragStart.current.x),
        y: dragStart.current.oy + (e.clientY - dragStart.current.y),
      });
    }
  }

  function onPointerUp(e: React.PointerEvent) {
    pointers.current.delete(e.pointerId);
    if (pointers.current.size < 2) pinchStart.current = null;
    if (pointers.current.size === 0) dragStart.current = null;
  }

  function zoomBy(factor: number) {
    setScale((current) => {
      const next = clamp(current * factor);
      if (next === 1) setOffset({ x: 0, y: 0 });
      return next;
    });
  }

  return createPortal(
    <div
      className="fixed inset-0 z-[999] flex flex-col bg-black"
      role="dialog"
      aria-modal="true"
      aria-label="Report preview"
    >
      <div
        className="flex shrink-0 items-center justify-between gap-2 px-3 py-2"
        style={{ paddingTop: 'calc(env(safe-area-inset-top, 0px) + 0.5rem)' }}
      >
        {/* Deliberately the biggest thing on the bar. Somebody who cannot find
            this is stuck inside a picture with their whole library behind it. */}
        <button
          type="button"
          onClick={onClose}
          aria-label="Close the preview"
          className="flex h-12 items-center gap-2 rounded-xl bg-white/15 px-4 text-base font-medium text-white active:bg-white/25"
        >
          <span aria-hidden className="text-xl leading-none">
            ✕
          </span>
          Close
        </button>

        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => zoomBy(1 / 1.5)}
            aria-label="Zoom out"
            className="size-12 rounded-xl bg-white/10 text-2xl text-white active:bg-white/25"
          >
            −
          </button>
          <span className="w-12 text-center text-xs text-white/70">{Math.round(scale * 100)}%</span>
          <button
            type="button"
            onClick={() => zoomBy(1.5)}
            aria-label="Zoom in"
            className="size-12 rounded-xl bg-white/10 text-2xl text-white active:bg-white/25"
          >
            +
          </button>
        </div>
      </div>

      <div
        className="min-h-0 flex-1 touch-none overflow-hidden"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onWheel={(e) => zoomBy(e.deltaY < 0 ? 1.15 : 1 / 1.15)}
        onDoubleClick={() => {
          setScale((s) => (s > 1 ? 1 : 2.5));
          setOffset({ x: 0, y: 0 });
        }}
      >
        {src ? (
          <img
            src={src}
            alt="The page as it was scanned"
            draggable={false}
            className="h-full w-full select-none object-contain"
            style={{
              transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
              transformOrigin: 'center',
              transition: dragStart.current || pinchStart.current ? 'none' : 'transform 120ms',
            }}
          />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-sm text-white/70">
            {!failed && (
              <span
                role="progressbar"
                aria-label="Opening the page"
                className="inline-block size-8 animate-spin rounded-full border-2 border-white/70 border-t-transparent"
              />
            )}
            <p>{failed ? 'This file cannot be shown.' : 'Opening the page…'}</p>
          </div>
        )}
      </div>

      <p
        className="shrink-0 px-4 pb-3 text-center text-xs text-white/50"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom, 0px) + 0.75rem)' }}
      >
        Pinch or scroll to zoom · drag to move · double-tap to reset
      </p>
    </div>,
    document.body,
  );
}
