import React from 'react';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import MemoryPanel from '../MemoryPanel';

// memory_type uses snake_case to match backend MemoryType serde representation
const twoMemories = [
  { id: '1', content: 'I prefer dark mode', memory_type: 'preference', tags: [], created_at: new Date().toISOString() },
  { id: '2', content: 'Works on AI projects', memory_type: 'fact', tags: ['work'], created_at: new Date().toISOString() },
];

describe('MemoryPanel', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it('shows loading state initially', () => {
    vi.mocked(invoke).mockReturnValue(new Promise(() => {}));
    render(<MemoryPanel onClose={vi.fn()} />);
    expect(screen.getByText(/loading/i)).toBeTruthy();
  });

  it('renders memories after load', async () => {
    vi.mocked(invoke).mockResolvedValue(twoMemories);
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

  it('deduplicates memories returned across type buckets', async () => {
    // All 4 get_memories_by_type calls return the same array — dedup should show each once
    vi.mocked(invoke).mockResolvedValue(twoMemories);
    render(<MemoryPanel onClose={vi.fn()} />);
    await waitFor(() => {
      expect(screen.getByText('I prefer dark mode')).toBeTruthy();
    });
    expect(screen.getAllByText('I prefer dark mode')).toHaveLength(1);
  });

  it('calls search_memories when user types in search box', async () => {
    // Initial load returns two memories; search returns one
    vi.mocked(invoke)
      .mockResolvedValueOnce(twoMemories)
      .mockResolvedValueOnce(twoMemories)
      .mockResolvedValueOnce(twoMemories)
      .mockResolvedValueOnce(twoMemories)
      .mockResolvedValueOnce([twoMemories[0]]); // search_memories result

    render(<MemoryPanel onClose={vi.fn()} />);
    await waitFor(() => screen.getByText('I prefer dark mode'));

    const searchInput = screen.getByPlaceholderText(/search memories/i);
    fireEvent.change(searchInput, { target: { value: 'dark' } });

    await waitFor(() => {
      const searchCall = vi.mocked(invoke).mock.calls.find(
        call => call[0] === 'search_memories'
      );
      expect(searchCall).toBeTruthy();
      expect(searchCall?.[1]).toMatchObject({ query: 'dark' });
    });
  });

  it('removes memory from list after delete', async () => {
    vi.mocked(invoke).mockResolvedValue(twoMemories);
    render(<MemoryPanel onClose={vi.fn()} />);
    await waitFor(() => screen.getByText('I prefer dark mode'));

    // Set up delete mock to succeed
    vi.mocked(invoke).mockResolvedValueOnce(undefined);

    const deleteButtons = screen.getAllByRole('button', { name: /delete memory/i });
    fireEvent.click(deleteButtons[0]);

    await waitFor(() => {
      expect(screen.queryByText('I prefer dark mode')).toBeNull();
    });
    expect(screen.getByText('Works on AI projects')).toBeTruthy();
  });
});
