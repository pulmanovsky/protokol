'use client';

/** One line of a native context menu; `separator` draws a line instead */
export type NativeMenuItem = { text: string; action: () => void; enabled?: boolean } | 'separator';

/**
 * Shows a native macOS context menu at the pointer (as Finder and Notes do).
 * Resolves false when it could not be shown (outside the app, or the menu
 * API refused), so the caller can show its own menu instead.
 */
export async function popupNativeMenu(items: NativeMenuItem[]): Promise<boolean> {
  try {
    const { Menu, MenuItem, PredefinedMenuItem } = await import('@tauri-apps/api/menu');
    const built = await Promise.all(
      items.map((it) =>
        it === 'separator'
          ? PredefinedMenuItem.new({ item: 'Separator' })
          : MenuItem.new({ text: it.text, enabled: it.enabled ?? true, action: () => it.action() }),
      ),
    );
    const menu = await Menu.new({ items: built });
    await menu.popup();
    return true;
  } catch (e) {
    console.warn('Native menu not shown', e);
    return false;
  }
}
