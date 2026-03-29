import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import ContextMenu from './ContextMenu';

// E-PENPAL-CONTEXT-MENU: tests for shared context menu component.
describe('ContextMenu', () => {
  it('renders menu items as buttons', () => {
    const onClick = vi.fn();
    render(<ContextMenu x={100} y={200} items={[{ label: 'Copy', onClick }, { label: 'Delete', onClick }]} onClose={vi.fn()} />);
    expect(screen.getByText('Copy')).toBeDefined();
    expect(screen.getByText('Delete')).toBeDefined();
  });

  it('renders dividers for separator items', () => {
    const { container } = render(
      <ContextMenu x={0} y={0} items={[{ label: 'A', onClick: vi.fn() }, { label: '---', onClick: vi.fn() }, { label: 'B', onClick: vi.fn() }]} onClose={vi.fn()} />,
    );
    expect(container.querySelectorAll('.menu-divider').length).toBe(1);
  });

  it('calls onClick and onClose when an item is clicked', () => {
    const onClick = vi.fn();
    const onClose = vi.fn();
    render(<ContextMenu x={0} y={0} items={[{ label: 'Action', onClick }]} onClose={onClose} />);
    fireEvent.click(screen.getByText('Action'));
    expect(onClick).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('calls onClose on Escape key', () => {
    const onClose = vi.fn();
    render(<ContextMenu x={0} y={0} items={[{ label: 'Item', onClick: vi.fn() }]} onClose={onClose} />);
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('calls onClose on click outside', () => {
    const onClose = vi.fn();
    render(<ContextMenu x={0} y={0} items={[{ label: 'Item', onClick: vi.fn() }]} onClose={onClose} />);
    fireEvent.mouseDown(document);
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('applies custom className to items', () => {
    render(
      <ContextMenu x={0} y={0} items={[{ label: 'Danger', className: 'menu-danger', onClick: vi.fn() }]} onClose={vi.fn()} />,
    );
    expect(screen.getByText('Danger').className).toContain('menu-danger');
  });
});
