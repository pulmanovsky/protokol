"use client"

import { useEffect, useState } from "react"
import { Switch } from "./ui/switch"
import { invoke } from "@tauri-apps/api/core"
import { SettingsRow } from "./settings/SettingsGroup"
import { clickPlaysWord, setClickPlaysWord } from "@/lib/clickWord"
import { WORD_HIGHLIGHT_EVENT } from "@/lib/lineTimes"

/** «Підсвічувати кожне слово»: the default depends on the Mac's memory; the choice is kept */
export function ClickWordRow() {
  const [on, setOn] = useState(true)
  useEffect(() => setOn(clickPlaysWord()), [])
  return (
    <SettingsRow
      label={
        <span className="flex flex-col min-w-0">
          <span>Прослуховувати слово за кліком</span>
          <span className="text-xs text-[color:var(--muted-text)]">Звучить саме це слово голосом того, хто його сказав. Вимкніть, щоб клік лише ставив курсор і переводив запис на це місце.</span>
        </span>
      }
    >
      <Switch
        checked={on}
        onCheckedChange={(v) => {
          setOn(v)
          setClickPlaysWord(v)
        }}
        aria-label="Прослуховувати слово за кліком"
      />
    </SettingsRow>
  )
}

export function WordHighlightRow() {
  const [on, setOn] = useState<boolean | null>(null)
  useEffect(() => {
    invoke<boolean>("api_get_word_highlight").then(setOn).catch(() => setOn(null))
  }, [])
  const change = async (value: boolean) => {
    setOn(value)
    try {
      await invoke("api_set_word_highlight", { on: value })
      window.dispatchEvent(new CustomEvent(WORD_HIGHLIGHT_EVENT, { detail: value }))
    } catch {
      setOn(!value)
    }
  }
  return (
    <SettingsRow
      label={
        <span className="flex flex-col min-w-0">
          <span>Підсвічувати кожне слово</span>
          <span className="text-xs text-[color:var(--muted-text)]">Вирівнює слова по звуку. Вимкніть, якщо Mac повільний: тоді підсвічується весь абзац.</span>
        </span>
      }
    >
      <Switch checked={!!on} disabled={on === null} onCheckedChange={(v) => void change(v)} aria-label="Підсвічувати кожне слово" />
    </SettingsRow>
  )
}
