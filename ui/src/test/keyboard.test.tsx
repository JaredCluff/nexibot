import { describe, it, expect, vi } from 'vitest';

function createKeyboardHandler(onNewChat: () => void, onFocusSearch: () => void) {
  return (e: KeyboardEvent) => {
    const isMac = navigator.platform.toUpperCase().includes('MAC');
    const ctrl = isMac ? e.metaKey : e.ctrlKey;
    if (!ctrl) return;
    if (e.key === 'k') { e.preventDefault(); onNewChat(); }
    if (e.key === '/') { e.preventDefault(); onFocusSearch(); }
  };
}

describe('keyboard shortcuts', () => {
  it('Ctrl+K calls onNewChat', () => {
    const onNewChat = vi.fn();
    const handler = createKeyboardHandler(onNewChat, vi.fn());
    // jsdom navigator.platform is empty, so isMac=false and we use ctrlKey
    handler(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true }));
    expect(onNewChat).toHaveBeenCalledOnce();
  });

  it('Ctrl+/ calls onFocusSearch', () => {
    const onFocusSearch = vi.fn();
    const handler = createKeyboardHandler(vi.fn(), onFocusSearch);
    handler(new KeyboardEvent('keydown', { key: '/', ctrlKey: true }));
    expect(onFocusSearch).toHaveBeenCalledOnce();
  });

  it('non-modifier key does nothing', () => {
    const onNewChat = vi.fn();
    const handler = createKeyboardHandler(onNewChat, vi.fn());
    handler(new KeyboardEvent('keydown', { key: 'k', metaKey: false, ctrlKey: false }));
    expect(onNewChat).not.toHaveBeenCalled();
  });

  it('Cmd+K calls onNewChat on Mac (metaKey)', () => {
    const onNewChat = vi.fn();
    const handler = createKeyboardHandler(onNewChat, vi.fn());
    // Simulate Mac behavior by directly testing metaKey path
    const originalPlatform = navigator.platform;
    Object.defineProperty(navigator, 'platform', { value: 'MacIntel', configurable: true });
    handler(new KeyboardEvent('keydown', { key: 'k', metaKey: true }));
    expect(onNewChat).toHaveBeenCalledOnce();
    Object.defineProperty(navigator, 'platform', { value: originalPlatform, configurable: true });
  });
});
