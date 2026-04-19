import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import ExecutionSummaryBadge from '../ExecutionSummaryBadge';

describe('ExecutionSummaryBadge', () => {
  const summary = {
    iterations_used: 3,
    elapsed_ms: 2400,
    tools_called: ['nexibot_bash', 'nexibot_file_read', 'nexibot_bash'],
    fallbacks: [],
  };

  it('renders elapsed time', () => {
    render(<ExecutionSummaryBadge summary={summary} />);
    expect(screen.getByText(/2\.4s/)).toBeTruthy();
  });

  it('renders iteration count', () => {
    render(<ExecutionSummaryBadge summary={summary} />);
    expect(screen.getByText(/3 steps/)).toBeTruthy();
  });

  it('expands to show tool list on click', () => {
    render(<ExecutionSummaryBadge summary={summary} />);
    const badge = screen.getByRole('button');
    expect(screen.queryByText('nexibot_bash')).toBeNull();
    fireEvent.click(badge);
    expect(screen.getByText(/nexibot_bash \(×2\)/)).toBeTruthy();
    expect(screen.getByText(/nexibot_file_read/)).toBeTruthy();
  });
});
