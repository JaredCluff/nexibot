import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import CodeBlock from './CodeBlock';

describe('CodeBlock', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders code content', () => {
    render(<CodeBlock code="console.log('hello');" />);
    expect(screen.getByText("console.log('hello');")).toBeInTheDocument();
  });

  it('shows language label when provided', () => {
    render(<CodeBlock code="let x = 1;" language="typescript" />);
    expect(screen.getByText('typescript')).toBeInTheDocument();
  });

  it('copy button copies code to clipboard', async () => {
    const writeTextSpy = vi.fn(() => Promise.resolve());
    Object.assign(navigator, {
      clipboard: { writeText: writeTextSpy, readText: vi.fn(() => Promise.resolve('')) },
    });
    const code = 'const a = 42;';
    render(<CodeBlock code={code} />);

    const copyBtn = screen.getByRole('button', { name: /copy/i });
    await act(async () => {
      fireEvent.click(copyBtn);
    });

    await waitFor(() => {
      expect(writeTextSpy).toHaveBeenCalledWith(code);
    });
  });

  it('shows copied feedback after clicking copy', async () => {
    const user = userEvent.setup();
    render(<CodeBlock code="x" />);

    const copyBtn = screen.getByRole('button', { name: /copy/i });
    await user.click(copyBtn);

    expect(screen.getByText('Copied!')).toBeInTheDocument();
  });

  it('shows Open in Canvas button when callback provided', () => {
    const onOpen = vi.fn();
    render(<CodeBlock code="x" onOpenInCanvas={onOpen} />);

    expect(screen.getByRole('button', { name: /open in canvas/i })).toBeInTheDocument();
  });

  it('hides Open in Canvas button when no callback', () => {
    render(<CodeBlock code="x" />);

    expect(screen.queryByRole('button', { name: /open in canvas/i })).not.toBeInTheDocument();
  });

  it('clicking Open in Canvas calls callback with code and language', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    render(<CodeBlock code="fn main() {}" language="rust" onOpenInCanvas={onOpen} />);

    const btn = screen.getByRole('button', { name: /open in canvas/i });
    await user.click(btn);

    expect(onOpen).toHaveBeenCalledWith('fn main() {}', 'rust');
  });
});
