import React from 'react';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import HistorySidebar from '../HistorySidebar';

const mockSessions = [
  {
    id: '1',
    title: 'Rust async patterns',
    started_at: new Date().toISOString(),
    last_activity: new Date().toISOString(),
    messages: [],
  },
  {
    id: '2',
    title: 'TypeScript generics',
    started_at: new Date().toISOString(),
    last_activity: new Date().toISOString(),
    messages: [],
  },
  {
    id: '3',
    title: 'React hooks tutorial',
    started_at: new Date().toISOString(),
    last_activity: new Date().toISOString(),
    messages: [],
  },
];

describe('HistorySidebar search', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockResolvedValue(mockSessions);
  });

  it('shows all sessions when search is empty', async () => {
    render(
      <HistorySidebar
        isOpen={true}
        onToggle={vi.fn()}
        onSessionSelect={vi.fn()}
        onNewConversation={vi.fn()}
        currentSessionId={undefined}
      />
    );
    await waitFor(() => {
      expect(screen.getByText('Rust async patterns')).toBeTruthy();
    });
    expect(screen.getByText('TypeScript generics')).toBeTruthy();
    expect(screen.getByText('React hooks tutorial')).toBeTruthy();
  });

  it('filters sessions by title', async () => {
    render(
      <HistorySidebar
        isOpen={true}
        onToggle={vi.fn()}
        onSessionSelect={vi.fn()}
        onNewConversation={vi.fn()}
        currentSessionId={undefined}
      />
    );
    await waitFor(() => {
      expect(screen.getByText('Rust async patterns')).toBeTruthy();
    });
    const searchInput = screen.getByPlaceholderText(/search/i);
    fireEvent.change(searchInput, { target: { value: 'typescript' } });
    expect(screen.getByText('TypeScript generics')).toBeTruthy();
    expect(screen.queryByText('Rust async patterns')).toBeNull();
    expect(screen.queryByText('React hooks tutorial')).toBeNull();
  });

  it('shows no-results message when nothing matches', async () => {
    render(
      <HistorySidebar
        isOpen={true}
        onToggle={vi.fn()}
        onSessionSelect={vi.fn()}
        onNewConversation={vi.fn()}
        currentSessionId={undefined}
      />
    );
    await waitFor(() => {
      expect(screen.getByText('Rust async patterns')).toBeTruthy();
    });
    const searchInput = screen.getByPlaceholderText(/search/i);
    fireEvent.change(searchInput, { target: { value: 'xyznotexist' } });
    expect(screen.getByText(/no conversations/i)).toBeTruthy();
  });
});
