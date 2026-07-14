import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';

describe('App', () => {
  it('renders the product name and version', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: 'Wuwa ini Tool' })).toBeVisible();
    expect(screen.getByText('1.0.0')).toBeVisible();
  });
});
