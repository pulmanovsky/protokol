interface OnboardingSummaryModelStatusInput {
  selectedModel: string;
  recommendedModel: string;
  selectedModelReady: boolean;
}

interface OnboardingSummaryModelStatus {
  selectedSummaryModel: string;
  summaryModelDownloaded: boolean;
}

// Fallback only: the real sizes come from the backend catalog and the download's Content-Length
const SUMMARY_MODEL_SIZES_MB: Record<string, number> = {
  'mamaylm:12b': 6962,
  'gemma3:12b': 6962,
};

/** Display names for download toasts */
const SUMMARY_MODEL_NAMES: Record<string, string> = {
  'mamaylm:12b': 'MamayLM 12B',
  'gemma3:12b': 'Gemma 3 12B',
};

export function getSummaryModelDisplayName(model: string): string {
  return SUMMARY_MODEL_NAMES[model] ?? model;
}

export function resolveOnboardingSummaryModelStatus({
  selectedModel,
  recommendedModel,
  selectedModelReady,
}: OnboardingSummaryModelStatusInput): OnboardingSummaryModelStatus {
  const selectedSummaryModel = selectedModel || recommendedModel;

  return {
    selectedSummaryModel,
    summaryModelDownloaded: Boolean(selectedSummaryModel && selectedModelReady),
  };
}

export function getSummaryModelSizeMb(model: string): number {
  return SUMMARY_MODEL_SIZES_MB[model] ?? 0;
}

export function getDownloadTotalMb(totalMb: number | null | undefined, model: string): number {
  return totalMb || getSummaryModelSizeMb(model);
}

export function formatSummaryModelSizeLabelFromMb(sizeMb: number): string {
  if (sizeMb === 0) {
    return '';
  }

  if (sizeMb >= 1024) {
    return `~${(sizeMb / 1024).toFixed(1).replace('.', ',')} ГБ`;
  }

  return `~${sizeMb} МБ`;
}

export function getSummaryModelSizeLabel(model: string): string {
  return formatSummaryModelSizeLabelFromMb(getSummaryModelSizeMb(model));
}
