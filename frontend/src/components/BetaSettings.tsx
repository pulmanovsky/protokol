"use client"

import { Switch } from "./ui/switch"
import { SettingsGroup, SettingsRow } from "./settings/SettingsGroup"
import { useConfig } from "@/contexts/ConfigContext"
import {
  BetaFeatureKey,
  BETA_FEATURE_NAMES,
  BETA_FEATURE_DESCRIPTIONS
} from "@/types/betaFeatures"

export function BetaSettings() {
  const { betaFeatures, toggleBetaFeature } = useConfig();

  // Define feature order for display (allows custom ordering)
  const featureOrder: BetaFeatureKey[] = ['importAndRetranscribe'];

  return (
    <SettingsGroup help="Ці функції ще тестуються та можуть працювати нестабільно. Якщо вимкнути, вони зникнуть з інтерфейсу; наявні записи це не змінить.">
      {featureOrder.map((featureKey) => (
        <SettingsRow key={featureKey} label={BETA_FEATURE_NAMES[featureKey]}>
          <Switch
            checked={betaFeatures[featureKey]}
            onCheckedChange={(checked) => toggleBetaFeature(featureKey, checked)}
            aria-label={BETA_FEATURE_NAMES[featureKey]}
          />
        </SettingsRow>
      ))}
    </SettingsGroup>
  );
}
