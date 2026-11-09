import React, { useState, useEffect } from "react";
import { getVersion } from '@tauri-apps/api/app';
import Image from 'next/image';

// Updates are installed manually for now: the app makes no automatic network requests
export function About() {
    const [currentVersion, setCurrentVersion] = useState<string>('0.2.3');

    useEffect(() => {
        getVersion().then(setCurrentVersion).catch(console.error);
    }, []);

    return (
        <div className="max-w-xl space-y-6">
            <div className="flex items-center gap-4">
                <Image
                    src="icon_128x128.png"
                    alt=""
                    width={64}
                    height={64}
                    className="rounded-[14px]"
                />
                <div className="flex flex-col gap-1">
                    <h2 className="text-xl font-bold text-gray-900">Протокол</h2>
                    <span className="text-sm text-gray-500">Версія {currentVersion}</span>
                </div>
            </div>
            <p className="text-sm text-gray-600 leading-relaxed">
                Запис розмов, стенограма та документ українською. Розпізнавання мовлення та підсумки працюють локально на вашому комп'ютері.
                Застосунок нічого не надсилає назовні без вашої дії.
            </p>
            <p className="pt-4 border-t border-gray-200 text-xs text-gray-500">
                Створено на основі відкритого проєкту Meetily (Zackriya Solutions).
            </p>
        </div>
    )
}
