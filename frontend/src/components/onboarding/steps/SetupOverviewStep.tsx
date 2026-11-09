import React, { useEffect, useState } from 'react';
import { Info } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { importModelsFromFolder } from '@/lib/importModels';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

export function SetupOverviewStep() {
  const { goNext } = useOnboarding();
  const [isMac, setIsMac] = useState(false);

  useEffect(() => {
    const checkPlatform = async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch (e) {
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };
    checkPlatform();
  }, []);

  const steps = [
    {
      number: 1,
      type: 'transcription',
      title: 'Завантажити розпізнавання мовлення',
    },
    {
      number: 2,
      type: 'summarization',
      title: 'Завантажити модель для підсумків',
    },
  ];

  const handleContinue = () => {
    goNext();
  };

  return (
    <OnboardingContainer
      title="Що потрібно для роботи"
      description="Протокол працює локально, тому спершу завантажимо дві моделі: для розпізнавання мовлення і для складання підсумків."
      step={2}
      totalSteps={isMac ? 4 : 3}
    >
      <div className="flex flex-col items-center space-y-10">
        {/* Steps Card */}
        <div className="card w-full max-w-md p-5">
          <div className="space-y-4">
            {steps.map((step, idx) => {
              return (
                <div
                  key={step.number}
                  className={`flex items-start gap-4 p-1`}
                >
                  <div className="flex-1 ml-1">
                    <h3 className="font-medium text-gray-900 flex items-center gap-2">
                        Крок {step.number}. {step.title}

                        {step.type === "summarization" && (
                            <TooltipProvider>
                            <Tooltip>
                                <TooltipTrigger asChild>
                                <button className="text-gray-400 hover:text-gray-600">
                                    <Info className="w-4 h-4" />
                                </button>
                                </TooltipTrigger>
                                <TooltipContent className="max-w-xs text-sm">
                                У налаштуваннях можна обрати й зовнішній сервіс (OpenAI, Claude,
                                Ollama). Тоді текст стенограми надсилатиметься цьому сервісу.
                                </TooltipContent>
                            </Tooltip>
                            </TooltipProvider>
                        )}
                        </h3>
                  </div>
                </div>
              );
            })}
          </div>
        </div>


        {/* CTA Section */}
        <div className="w-full max-w-xs space-y-4">
          <Button
            onClick={handleContinue}
            className="accent-button w-full h-11 text-[15px] font-semibold"
          >
            Далі
          </Button>
          {/* Without the internet: the models from a folder (a USB stick) first, then «Далі» finds them in place */}
          <button
            type="button"
            className="w-full text-center text-[13px] text-[color:var(--muted-text)] hover:underline"
            onClick={() => void importModelsFromFolder()}
          >
            Без інтернету? Встановити моделі з папки…
          </button>
        </div>
      </div>
    </OnboardingContainer>
  );
}
