import { invoke } from '@tauri-apps/api/core';

export async function copyText(text: string): Promise<void> {
  try {
    await invoke('copy_to_clipboard', { text });
    return;
  } catch (error) {
    console.warn('Tauri clipboard failed, trying browser clipboard', error);
  }

  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  throw new Error('clipboard unavailable');
}
