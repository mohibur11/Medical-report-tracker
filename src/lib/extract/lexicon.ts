/**
 * Recognising what a document IS, from the text on it.
 *
 * Two questions, answered together because one lookup settles both:
 *   - what to call it (the title, which becomes part of the filename)
 *   - what kind of thing it is (report, prescription, invoice)
 *
 * This is deliberately a curated lexicon rather than a general extractor. The
 * plan is blunt about why: a test name is not extractable in general. What works
 * is matching against the tests a person actually has done, and South Asian and
 * South-East Asian lab reports use a stable vocabulary — CBC, ESR, FBS, HbA1c,
 * S. Creatinine, SGPT, TSH, FT4, USG of Whole Abdomen.
 *
 * Every entry carries a category hint, so tagging and titling come from the same
 * lookup and the user confirms one suggestion instead of two.
 *
 * Suggestions only. The review row pre-fills them and the user corrects; nothing
 * here is ever applied silently.
 */

export type DocType = 'report' | 'prescription' | 'invoice' | 'other';

export interface LexiconEntry {
  /** Canonical title, written the way a person would want to read it. */
  title: string;
  /** Lower-cased phrases that identify it. Longer phrases score higher. */
  match: string[];
  /** Suggested category, so one lookup drives the tag too. */
  category?: string;
}

/**
 * Curated, not exhaustive. Ordered loosely by how often these turn up in a
 * personal archive; ordering does not affect scoring.
 */
export const LEXICON: LexiconEntry[] = [
  // Thyroid
  { title: 'Thyroid Profile', match: ['thyroid profile', 'thyroid function'], category: 'Thyroid' },
  { title: 'TSH', match: ['thyroid stimulating hormone', 'tsh'], category: 'Thyroid' },
  { title: 'Free Thyroxine (FT4)', match: ['free thyroxine', 'ft4'], category: 'Thyroid' },
  { title: 'Free T3 (FT3)', match: ['free triiodothyronine', 'ft3'], category: 'Thyroid' },
  { title: 'Thyroglobulin Antibody', match: ['thyroglobulin antibody', 'anti-tg'], category: 'Thyroid' },
  { title: 'Anti-TPO', match: ['microsomal antibody', 'anti-tpo', 'anti tpo'], category: 'Thyroid' },
  { title: 'Calcitonin', match: ['calcitonin'], category: 'Thyroid' },
  { title: 'USG of Thyroid', match: ['usg of thyroid', 'ultrasonogram of thyroid', 'thyroid gland'], category: 'Thyroid' },
  { title: 'Thyroid Scan', match: ['thyroid scan', 'thyroid scintigraphy'], category: 'Thyroid' },
  { title: 'FNA Thyroid', match: ['fna thyroid', 'fine needle aspiration'], category: 'Thyroid' },

  // Haematology
  { title: 'Complete Blood Count', match: ['complete blood count', 'cbc', 'full blood count'] },
  { title: 'ESR', match: ['erythrocyte sedimentation', 'esr'] },
  { title: 'Haemoglobin', match: ['haemoglobin', 'hemoglobin', 'hb%'] },
  { title: 'Blood Grouping', match: ['blood group', 'abo grouping'] },
  { title: 'Prothrombin Time', match: ['prothrombin time', 'pt inr', 'inr'] },

  // Diabetes and metabolic
  { title: 'Fasting Blood Sugar', match: ['fasting blood sugar', 'fbs', 'fasting glucose'], category: 'Diabetes' },
  { title: 'HbA1c', match: ['hba1c', 'glycated haemoglobin', 'glycosylated'], category: 'Diabetes' },
  { title: 'OGTT', match: ['oral glucose tolerance', 'ogtt'], category: 'Diabetes' },
  { title: 'Lipid Profile', match: ['lipid profile', 'cholesterol', 'triglyceride'], category: 'Cardiac' },

  // Kidney and liver
  { title: 'Serum Creatinine', match: ['serum creatinine', 's. creatinine', 'creatinine'], category: 'Kidney' },
  { title: 'Blood Urea', match: ['blood urea', 'urea nitrogen', 'bun'], category: 'Kidney' },
  { title: 'Urine R/E', match: ['urine r/e', 'urine routine', 'urine analysis'], category: 'Kidney' },
  { title: 'Liver Function Test', match: ['liver function', 'lft'], category: 'Liver' },
  { title: 'SGPT (ALT)', match: ['sgpt', 'alanine aminotransferase', 'alt '], category: 'Liver' },
  { title: 'SGOT (AST)', match: ['sgot', 'aspartate aminotransferase'], category: 'Liver' },
  { title: 'Serum Bilirubin', match: ['bilirubin'], category: 'Liver' },

  // Cardiac
  { title: 'ECG', match: ['electrocardiogram', 'ecg', 'ekg'], category: 'Cardiac' },
  { title: 'Echocardiogram', match: ['echocardiogram', 'echo cardiography', '2d echo'], category: 'Cardiac' },
  { title: 'Troponin I', match: ['troponin'], category: 'Cardiac' },

  // Imaging
  { title: 'USG of Whole Abdomen', match: ['usg of whole abdomen', 'ultrasonogram of whole abdomen', 'whole abdomen'] },
  { title: 'Ultrasonogram', match: ['ultrasonogram', 'ultrasound', 'usg of', 'sonography'] },
  { title: 'Chest X-Ray', match: ['chest x-ray', 'x-ray chest', 'cxr'] },
  { title: 'CT Scan', match: ['ct scan', 'computed tomography'] },
  { title: 'MRI', match: ['mri', 'magnetic resonance'] },
  { title: 'Mammogram', match: ['mammogram', 'mammography'] },

  // Pathology and other
  { title: 'Pathological Report', match: ['pathological report', 'histopathology', 'cytology'] },
  { title: 'Biopsy Report', match: ['biopsy'] },
  { title: 'Allergy Panel', match: ['allergy', 'specific ige', 'allergen'] },
  { title: 'Serum Electrolytes', match: ['electrolytes', 'sodium potassium'] },
  { title: 'Vitamin D', match: ['vitamin d', '25-oh'] },
  { title: 'Serum Calcium', match: ['serum calcium'] },
  { title: 'Dengue NS1', match: ['dengue', 'ns1 antigen'] },
  { title: 'Widal Test', match: ['widal'] },
  { title: 'CRP', match: ['c-reactive protein', 'crp'] },
  { title: 'COVID-19 RT-PCR', match: ['rt-pcr', 'covid'] },

  // Non-test documents that still need a sensible title
  { title: 'Medical Report', match: ['medical report', 'discharge summary', 'consultation note'] },
  { title: 'Prescription', match: ['prescription'] },
  { title: 'Receipt', match: ['receipt', 'invoice', 'cashier', 'payment type'] },
  { title: 'Visit Slip', match: ['visit slip', 'appointment slip', 'queue number'] },
];

export interface Suggestion {
  title: string;
  category?: string;
  /** Which phrase matched, so the user can see why it was suggested. */
  matched: string;
}

/**
 * Suggest a title from recognised text.
 *
 * Longer phrases win, because "thyroid profile" is a better answer than the "tsh"
 * that also appears in the reference-range column. Short codes are only accepted
 * as whole words, or "alt" would match "alternate" and "ct" would match half the
 * page.
 */
export function suggestTitles(text: string, limit = 3): Suggestion[] {
  const hay = ` ${text.toLowerCase().replace(/\s+/g, ' ')} `;
  const hits: Array<Suggestion & { weight: number }> = [];

  for (const entry of LEXICON) {
    let best: { phrase: string; weight: number } | null = null;

    for (const phrase of entry.match) {
      const p = phrase.toLowerCase();
      // Anything this short is an abbreviation and must stand alone.
      const found = p.length <= 4
        ? new RegExp(`(?:^|[^a-z0-9])${escapeRegExp(p.trim())}(?:[^a-z0-9]|$)`, 'i').test(hay)
        : hay.includes(p);

      if (found && (!best || p.length > best.weight)) {
        best = { phrase, weight: p.length };
      }
    }

    if (best) {
      hits.push({ title: entry.title, category: entry.category, matched: best.phrase, weight: best.weight });
    }
  }

  hits.sort((a, b) => b.weight - a.weight);
  return hits.slice(0, limit).map(({ title, category, matched }) => ({ title, category, matched }));
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Work out what kind of document this is.
 *
 * A prescription is detected by its dosing notation rather than its words: a
 * doctor's slip may be almost entirely unreadable, but "1+0+1", "Tab.", "BD" and
 * "TDS" survive poor recognition and mean only one thing. That matters because a
 * prescription has no test name at all — it has drugs and a doctor — so title
 * matching should not be attempted on it.
 */
export function detectDocType(text: string): DocType {
  const t = text.toLowerCase();

  const invoiceSignals = [
    'receipt', 'invoice', 'cashier', 'payment type', 'amount', 'discount',
    'total', 'paid', 'baht', 'taka', 'vat', 'tax payer',
  ].filter((s) => t.includes(s)).length;

  const prescriptionSignals =
    (/\b\d\s*\+\s*\d\s*\+\s*\d\b/.test(t) ? 2 : 0) +
    (/\b(tab|cap|syp|inj)\.?\s/.test(t) ? 1 : 0) +
    (/\b(bd|tds|od|hs|prn|po|stat)\b/.test(t) ? 1 : 0) +
    (/\brx\b/.test(t) ? 1 : 0) +
    (t.includes('prescription') ? 1 : 0);

  // Receipts routinely mention "Rx dispensed" and list medicines, so they have to
  // outweigh the prescription signals rather than merely tie with them.
  if (invoiceSignals >= 3 && invoiceSignals >= prescriptionSignals) return 'invoice';
  if (prescriptionSignals >= 2) return 'prescription';

  const reportSignals = [
    'report', 'findings', 'impression', 'specimen', 'examination',
    'reference', 'result', 'diagnosis', 'lab no', 'pathologist',
    // Radiology and immunology reports use none of the words above. Real
    // ultrasound reports were being classed as "other" without these.
    'radiology', 'imaging', 'sonologist', 'echotexture', 'hypoechoic',
    'lobe', 'ultrasonogram', 'usg of', 'antigen', 'antibody', 'titer', 'titre',
  ].filter((s) => t.includes(s)).length;
  if (reportSignals >= 2) return 'report';

  return 'other';
}

/**
 * What the document is and what to call it, decided together.
 *
 * Order matters: the kind constrains the title. A hospital receipt lists the
 * medicines dispensed and prints "Prescription No.", so left to itself the title
 * matcher calls it a prescription; an itemised cashier sheet names every lab test
 * on the bill and gets titled after one of them. Neither is what the document is.
 */
export function describe(text: string): { docType: DocType; titles: Suggestion[] } {
  const docType = detectDocType(text);

  if (docType === 'prescription') {
    // Drugs and a doctor, not a test. Any title match here is noise.
    return { docType, titles: [] };
  }

  if (docType === 'invoice') {
    return { docType, titles: [{ title: 'Receipt', matched: 'receipt' }] };
  }

  return { docType, titles: suggestTitles(text) };
}
