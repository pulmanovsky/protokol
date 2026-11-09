import React from 'react';
import { Lock, Sparkles, Cpu } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';

export function WelcomeStep() {
  const { goNext } = useOnboarding();

  const features = [
    {
      icon: Lock,
      title: 'Записи й тексти залишаються на вашому Mac',
    },
    {
      icon: Sparkles,
      title: 'Стенограма та документ українською: рішення, завдання, відкриті питання',
    },
    {
      icon: Cpu,
      title: 'Працює без інтернету, без хмари',
    },
  ];

  return (
    <OnboardingContainer
      title="Ласкаво просимо до Протоколу"
      description="Запис, стенограма та документ на вашому комп'ютері."
      step={1}
      hideProgress={true}
    >
      <div className="flex flex-col items-center space-y-10">
        {/* Divider */}
        <div className="w-16 h-px bg-[var(--hairline)]" />

        {/* Features Card */}
        <div className="card w-full max-w-md p-6 space-y-4">
          {features.map((feature, index) => {
            const Icon = feature.icon;
            return (
              <div key={index} className="flex items-start gap-3">
                <div className="flex-shrink-0 mt-0.5">
                  <div className="w-5 h-5 rounded-full bg-gray-100 flex items-center justify-center">
                    <Icon className="w-3 h-3 text-gray-700" />
                  </div>
                </div>
                <p className="text-sm text-gray-700 leading-relaxed">{feature.title}</p>
              </div>
            );
          })}
        </div>

        {/* CTA Section */}
        <div className="w-full max-w-xs space-y-3">
          <Button
            onClick={goNext}
            className="accent-button w-full h-11 text-[15px] font-semibold"
          >
            Почати
          </Button>
          <p className="text-xs text-center text-[color:var(--muted-text)] text-balance">
            Один раз програма завантажить моделі, близько 10 ГБ. Скільки це триватиме, залежить від швидкості інтернету. Без інтернету моделі можна поставити з папки.
          </p>
        </div>
      </div>
    </OnboardingContainer>
  );
}
