import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import SvgPreview from './SvgPreview';

describe('SvgPreview', () => {
  it('renders SVG content in preview mode', () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg"><circle cx="50" cy="50" r="40"/></svg>';
    const { container } = render(<SvgPreview content={svg} />);

    const rendered = container.querySelector('.svg-preview-rendered');
    expect(rendered).toBeInTheDocument();
    expect(rendered!.innerHTML).toContain('circle');
  });

  it('toggle switches to source view', async () => {
    const user = userEvent.setup();
    const svg = '<svg><rect width="10" height="10"/></svg>';
    const { container } = render(<SvgPreview content={svg} />);

    const sourceBtn = screen.getByText('Source');
    await user.click(sourceBtn);

    const sourceBlock = container.querySelector('.svg-preview-source code');
    expect(sourceBlock).toBeInTheDocument();
    expect(sourceBlock!.textContent).toContain('<svg>');
  });

  it('sanitizes script tags from SVG', () => {
    const svg = '<svg><circle r="10"/><script>alert(1)</script></svg>';
    const { container } = render(<SvgPreview content={svg} />);

    const rendered = container.querySelector('.svg-preview-rendered');
    expect(rendered!.innerHTML).not.toContain('<script>');
    expect(rendered!.innerHTML).not.toContain('alert');
  });

  it('sanitizes event handlers from SVG', () => {
    const svg = '<svg><circle r="10" onclick="alert(1)"/></svg>';
    const { container } = render(<SvgPreview content={svg} />);

    const rendered = container.querySelector('.svg-preview-rendered');
    expect(rendered!.innerHTML).not.toContain('onclick');
  });

  it('sanitizes javascript URLs from SVG', () => {
    const svg = '<svg><a href="javascript:alert(1)"><text>click</text></a></svg>';
    const { container } = render(<SvgPreview content={svg} />);

    const rendered = container.querySelector('.svg-preview-rendered');
    expect(rendered!.innerHTML).not.toContain('javascript:');
  });
});
