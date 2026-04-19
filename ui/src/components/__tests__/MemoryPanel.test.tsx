import React from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import MemoryPanel from '../MemoryPanel';

describe('MemoryPanel', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it('shows loading state initially', () => {
    // Never resolve so we stay in loading
    vi.mocked(invoke).mockReturnValue(new Promise(() => {}));
    render(<MemoryPanel onClose={vi.fn()} />);
    expect(screen.getByText(/loading/i)).toBeTruthy();
  });

  it('renders memories after load', async () => {
    const memories = [
      { id: '1', content: 'I prefer dark mode', memory_type: 'Preference', tags: [], created_at: new Date().toISOString() },
      { id: '2', content: 'Works on AI projects', memory_type: 'Fact', tags: ['work'], created_at: new Date().toISOString() },
    ];
    vi.mocked(invoke).mockResolvedValue(memories);
    render(<MemoryPanel onClose={vi.fn()} />);
    await waitFor(() => {
      expect(screen.getByText('I prefer dark mode')).toBeTruthy();
    });
    expect(screen.getByText('Works on AI projects')).toBeTruthy();
  });

  it('shows empty state when no memories', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    render(<MemoryPanel onClose={vi.fn()} />);
    await waitFor(() => {
      expect(screen.getByText(/no memories/i)).toBeTruthy();
    });
  });
});
