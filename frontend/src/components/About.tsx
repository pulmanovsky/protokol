import React, { useState, useEffect } from "react";
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import Image from 'next/image';
import { UpdateSettings } from './settings/UpdateSettings';
import { SettingsGroup, SettingsLinkRow } from './settings/SettingsGroup';
import { FEEDBACK_REPO, openFeedback } from '@/lib/feedback';

const openLink = (url: string) => void invoke('open_external_url', { url }).catch(() => {});

/** «Про програму»: the version with its updates, feedback and where to read more (as About + Software Update on a Mac) */
export function About({ checkNow = false }: { checkNow?: boolean }) {
    const [currentVersion, setCurrentVersion] = useState<string>('');

    useEffect(() => {
        getVersion().then(setCurrentVersion).catch(console.error);
    }, []);

    return (
        <div className="flex flex-col gap-6">
            <div className="flex flex-col gap-3">
                <div className="flex items-center gap-4">
                    <Image
                        src="icon_128x128.png"
                        alt=""
                        width={64}
                        height={64}
                        className="rounded-[14px]"
                    />
                    <div className="flex flex-col gap-1">
                        <span className="text-xl font-bold text-gray-900">Протокол</span>
                        <span className="text-sm text-gray-500 tabular-nums">Версія {currentVersion}</span>
                    </div>
                </div>
                <p className="text-sm text-gray-600 leading-relaxed">
                    Запис розмов, стенограма та документ українською. Розпізнавання мовлення та документ створюються на цьому Mac.
                    Програма нічого не надсилає назовні без дії користувача.
                </p>
            </div>

            <UpdateSettings checkNow={checkNow} />

            <SettingsGroup
                title="Зворотний зв'язок"
                help="Перед надсиланням показується весь текст звернення. Тексти стенограм, назви записів та імена не додаються."
            >
                <SettingsLinkRow label="Повідомити про проблему…" onClick={() => openFeedback('problem')} />
                <SettingsLinkRow label="Запропонувати покращення…" onClick={() => openFeedback('idea')} />
            </SettingsGroup>

            <SettingsGroup
                title="Інформація"
                help="Створено на основі відкритого проєкту Meetily (Zackriya Solutions)."
            >
                <SettingsLinkRow label="Приватність і дані" external onClick={() => openLink(`${FEEDBACK_REPO}/blob/main/PRIVACY_POLICY.md`)} />
                <SettingsLinkRow label="Ліцензії відкритого коду" external onClick={() => openLink(`${FEEDBACK_REPO}/blob/main/THIRD_PARTY_NOTICES.md`)} />
                <SettingsLinkRow label="Сторінка проєкту на GitHub" external onClick={() => openLink(FEEDBACK_REPO)} />
            </SettingsGroup>
        </div>
    )
}
