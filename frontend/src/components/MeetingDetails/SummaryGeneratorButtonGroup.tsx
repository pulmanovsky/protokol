"use client";

import { ModelConfig, ModelSettingsModal } from '@/components/ModelSettingsModal';
import {
  Dialog,
  DialogContent,
  DialogTrigger,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"
import { ChevronDown, Cpu, Square } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { useState, useEffect, ReactNode } from 'react';

interface SummaryGeneratorButtonGroupProps {
  languageSlot?: ReactNode;
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  hasTranscripts?: boolean;
  hasSummary?: boolean;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
  /** Before the language: the template of the document */
  templateSlot?: ReactNode;
  /** The model's button in the toolbar (the dialog still opens when the model is missing) */
  showModelButton?: boolean;
}

export function SummaryGeneratorButtonGroup({
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  summaryStatus,
  hasTranscripts = true,
  hasSummary = false,
  isModelConfigLoading = false,
  onOpenModelSettings,
  languageSlot,
  templateSlot,
  showModelButton = true,
}: SummaryGeneratorButtonGroupProps) {
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);

  // Expose the function to open the modal via callback registration
  useEffect(() => {
    if (onOpenModelSettings) {
      // Register our open dialog function with the parent by calling the callback
      // This allows the parent to store a reference to this function
      const openDialog = () => {
        console.log('📱 Opening model settings dialog via callback');
        setSettingsDialogOpen(true);
      };

      // Call the parent's callback with our open function
      // Note: This assumes onOpenModelSettings accepts a function parameter
      // We'll need to adjust the signature
      onOpenModelSettings(openDialog);
    }
  }, [onOpenModelSettings]);

  if (!hasTranscripts) {
    return null;
  }

  const isGenerating = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';

  // «MamayLM-Gemma-3-12B…gguf» → «MamayLM», «gemma3:12b» → «gemma3»
  const modelName = (() => {
    const raw = (modelConfig.model ?? '').split(/[\\/]/).pop() ?? '';
    const short = raw.replace(/\.gguf$/i, '').split(/[:_-]/)[0];
    return short || 'Модель';
  })();

  return (
    <>
      {isGenerating && (
        <button
          type="button"
          className="tb-btn"
          onClick={() => {
            Analytics.trackButtonClick('stop_summary_generation', 'meeting_details');
            onStopGeneration();
          }}
          title="Зупинити створення документа"
        >
          <Square className="h-3.5 w-3.5" fill="currentColor" />
          <span className="tb-label">Зупинити</span>
        </button>
      )}

      {templateSlot}
      {languageSlot}

      <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
        {showModelButton && (
          <DialogTrigger asChild>
            <button type="button" className="tb-btn tb-side" title={modelConfig.model ? `Модель для документа: ${modelConfig.model}` : 'Модель для документа'}>
              <Cpu className="h-4 w-4" />
              <span className="tb-label max-w-[140px] truncate">{modelName}</span>
              <ChevronDown className="tb-chevron h-3.5 w-3.5" aria-hidden="true" />
            </button>
          </DialogTrigger>
        )}
        <DialogContent aria-describedby={undefined}>
          <VisuallyHidden>
            <DialogTitle>Модель для документа</DialogTitle>
          </VisuallyHidden>
          <ModelSettingsModal
            onSave={async (config) => {
              await onSaveModelConfig(config);
              setSettingsDialogOpen(false);
            }}
            modelConfig={modelConfig}
            setModelConfig={setModelConfig}
            skipInitialFetch={true}
            layout="dialog"
          />
        </DialogContent>
      </Dialog>
    </>
  );
}
