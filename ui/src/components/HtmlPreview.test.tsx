import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import HtmlPreview from './HtmlPreview';

describe('HtmlPreview', () => {
  it('renders iframe in preview mode', () => {
    const { container } = render(<HtmlPreview content="<h1>Hello</h1>" />);

    const iframe = container.querySelector('iframe');
    expect(iframe).toBeInTheDocument();
  });

  it('iframe has sandbox attribute', () => {
    const { container } = render(<HtmlPreview content="<p>test</p>" />);

    const iframe = container.querySelector('iframe');
    expect(iframe).toHaveAttribute('sandbox');
  });

  it('toggle switches to source view', async () => {
    const user = userEvent.setup();
    const html = '<div>Some HTML</div>';
    const { container } = render(<HtmlPreview content={html} />);

    const sourceBtn = screen.getByText('Source');
    await user.click(sourceBtn);

    const sourceBlock = container.querySelector('.html-preview-source code');
    expect(sourceBlock).toBeInTheDocument();
    expect(sourceBlock!.textContent).toContain('<div>Some HTML</div>');
  });

  it('shows title when provided', () => {
    render(<HtmlPreview content="<p>hi</p>" title="My Preview" />);

    expect(screen.getByText('My Preview')).toBeInTheDocument();
  });
});
