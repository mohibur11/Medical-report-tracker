/**
 * Reading PDFs in the window, with pdf.js.
 *
 * A PDF emailed straight from a lab carries its text exactly. Reading that text
 * is faster than recognising pixels and cannot misread a digit, so it is tried
 * before OCR and the recognizer only runs when it comes back empty.
 *
 * Thumbnails deliberately do NOT happen here — the sidecar renders page one
 * during ingest, which avoids sending a whole scanned PDF across the bridge for
 * a picture 220 pixels wide.
 *
 * The worker is bundled rather than fetched: the app runs under a policy that
 * allows nothing off-origin, and it has to work with no network at all.
 */

import * as pdfjs from 'pdfjs-dist';
import type { PDFDocumentProxy } from 'pdfjs-dist';

pdfjs.GlobalWorkerOptions.workerSrc = new URL(
  'pdfjs-dist/build/pdf.worker.min.mjs',
  import.meta.url,
).href;

/** Below this, a "text layer" is page furniture — a scanner's stamp, a page number. */
const MEANINGFUL_CHARS = 40;

/**
 * The loading task is kept alongside the document because it owns the worker —
 * releasing one without the other leaks a worker per PDF opened.
 */
async function open(bytes: Uint8Array): Promise<{
  doc: PDFDocumentProxy;
  close: () => Promise<void>;
}> {
  // A copy, because pdf.js takes ownership of the buffer it is given and the
  // caller may still want theirs.
  const task = pdfjs.getDocument({ data: new Uint8Array(bytes) });
  const doc = await task.promise;
  return { doc, close: () => task.destroy() };
}

export interface PageText {
  pageNo: number;
  text: string;
}

/**
 * Text already inside the PDF, one entry per page that has any.
 *
 * Pages are reported individually because the decision is per page, not per
 * document: a digital report with a scanned annexe should read the first exactly
 * and recognise only the rest.
 */
export async function extractText(bytes: Uint8Array): Promise<PageText[]> {
  const { doc, close } = await open(bytes);
  try {
    const pages: PageText[] = [];
    for (let n = 1; n <= doc.numPages; n += 1) {
      const page = await doc.getPage(n);
      const content = await page.getTextContent();
      const text = content.items
        .map((item) => ('str' in item ? item.str : ''))
        .join(' ')
        .replace(/\s+/g, ' ')
        .trim();
      page.cleanup();
      if (text.length >= MEANINGFUL_CHARS) pages.push({ pageNo: n, text });
    }
    return pages;
  } finally {
    await close();
  }
}
