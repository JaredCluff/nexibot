import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import Canvas, { Artifact } from './Canvas';

// Mock child components to isolate Canvas logic
vi.mock('./MermaidDiagram', () => ({
  default: ({ code, title }: { code: string; title?: string }) => (
    <div data-testid="mermaid-diagram">{title}{code}</div>
  ),
}));

const makeArtifact = (overrides: Partial<Artifact> = {}): Artifact => ({
  id: 'art-1',
  type: 'code',
  language: 'javascript',
  content: 'const x = 1;',
  title: 'Artifact 1',
  ...overrides,
});

describe('Canvas', () => {
  it('renders empty state when no artifacts', () => {
    render(<Canvas artifacts={[]} onClose={vi.fn()} onRemoveArtifact={vi.fn()} />);
    expect(screen.getByText('No artifacts yet.')).toBeInTheDocument();
  });

  it('renders tabs for each artifact', () => {
    const artifacts: Artifact[] = [
      makeArtifact({ id: '1', title: 'First' }),
      makeArtifact({ id: '2', title: 'Second' }),
    ];
    render(<Canvas artifacts={artifacts} onClose={vi.fn()} onRemoveArtifact={vi.fn()} />);

    expect(screen.getByText('First')).toBeInTheDocument();
    expect(screen.getByText('Second')).toBeInTheDocument();
  });

  it('clicking tab switches active content', async () => {
    const user = userEvent.setup();
    const artifacts: Artifact[] = [
      makeArtifact({ id: '1', title: 'First', content: 'content-one' }),
      makeArtifact({ id: '2', title: 'Second', content: 'content-two' }),
    ];
    render(<Canvas artifacts={artifacts} onClose={vi.fn()} onRemoveArtifact={vi.fn()} />);

    // First artifact is active by default
    expect(screen.getByText('content-one')).toBeInTheDocument();

    // Click the second tab label
    await user.click(screen.getByText('Second'));

    expect(screen.getByText('content-two')).toBeInTheDocument();
  });

  it('close tab button calls onRemoveArtifact', async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    const artifacts: Artifact[] = [makeArtifact({ id: 'abc', title: 'Tab1' })];
    render(<Canvas artifacts={artifacts} onClose={vi.fn()} onRemoveArtifact={onRemove} />);

    // The close button on a tab renders as x (\u00d7)
    const closeBtns = screen.getAllByTitle('Close tab');
    await user.click(closeBtns[0]);

    expect(onRemove).toHaveBeenCalledWith('abc');
  });

  it('close canvas button calls onClose', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Canvas artifacts={[]} onClose={onClose} onRemoveArtifact={vi.fn()} />);

    const closeBtn = screen.getByTitle('Close canvas');
    await user.click(closeBtn);

    expect(onClose).toHaveBeenCalled();
  });

  it('renders CodeBlock for code type', () => {
    const artifacts: Artifact[] = [makeArtifact({ type: 'code', content: 'hello code' })];
    const { container } = render(
      <Canvas artifacts={artifacts} onClose={vi.fn()} onRemoveArtifact={vi.fn()} />
    );

    expect(container.querySelector('.codeblock')).toBeInTheDocument();
  });

  it('renders HtmlPreview for html type', () => {
    const artifacts: Artifact[] = [
      makeArtifact({ id: 'h1', type: 'html', content: '<h1>Hi</h1>', title: 'HTML Thing' }),
    ];
    const { container } = render(
      <Canvas artifacts={artifacts} onClose={vi.fn()} onRemoveArtifact={vi.fn()} />
    );

    expect(container.querySelector('.html-preview')).toBeInTheDocument();
  });

  it('renders SvgPreview for svg type', () => {
    const artifacts: Artifact[] = [
      makeArtifact({
        id: 's1',
        type: 'svg',
        content: '<svg><circle r="10"/></svg>',
        title: 'SVG Thing',
      }),
    ];
    const { container } = render(
      <Canvas artifacts={artifacts} onClose={vi.fn()} onRemoveArtifact={vi.fn()} />
    );

    expect(container.querySelector('.svg-preview')).toBeInTheDocument();
  });
});
