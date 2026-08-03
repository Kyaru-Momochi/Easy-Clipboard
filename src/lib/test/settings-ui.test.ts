// @vitest-environment jsdom

import '@testing-library/jest-dom/vitest';

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import App from '../../App.svelte';
import SettingsView from '../components/SettingsView.svelte';
import { createSettingsStore } from '../stores/settings';
import type { AppSettings } from '../types';

const { invokeMock, listenMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  listenMock: vi.fn(async () => vi.fn())
}));

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string) => path),
  invoke: invokeMock
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: listenMock
}));

const settings: AppSettings = {
  historyLimit: 100,
  favoriteLimit: 20,
  maxItemBytes: 50 * 1024 * 1024,
  hotkey: 'Ctrl+Shift+V',
  theme: 'system',
  motionScale: 1,
  autostart: false,
  clearOnExit: false
};

function setup(overrides?: {
  saveSettings?: (value: AppSettings) => Promise<void>;
  confirm?: (message: string) => boolean;
}) {
  const backend = {
    getSettings: vi.fn(async () => settings),
    saveSettings: vi.fn(
      overrides?.saveSettings ??
        (async (_value: AppSettings) => undefined)
    )
  };
  const store = createSettingsStore(backend);
  const clearHistoryAction = vi.fn(async () => undefined);
  const getAppInfoAction = vi.fn(async () => ({
    version: '0.1.0',
    appDataDir: String.raw`C:\Users\Tester\AppData\Roaming\Easy Clipboard`
  }));
  render(SettingsView, {
    store,
    clearHistoryAction,
    getAppInfoAction,
    confirmAction: vi.fn(overrides?.confirm ?? (() => true))
  });
  return { backend, store, clearHistoryAction, getAppInfoAction };
}

async function ready(): Promise<void> {
  await screen.findByDisplayValue('Ctrl+Shift+V');
}

afterEach(() => {
  window.history.replaceState({}, '', '/');
  document.documentElement.removeAttribute('data-theme');
  document.documentElement.classList.remove('reduce-motion');
  document.documentElement.style.removeProperty('--motion-scale');
  vi.clearAllMocks();
});

describe('SettingsView', () => {
  it.each([1, 500])('accepts %i MB and sends exact bytes to the backend', async (limitMb) => {
    const { backend } = setup();
    await ready();

    await fireEvent.input(screen.getByLabelText('单次内容上限 (MB)'), {
      target: { value: String(limitMb) }
    });
    await fireEvent.click(screen.getByRole('button', { name: '保存设置' }));

    await waitFor(() => expect(backend.saveSettings).toHaveBeenCalledOnce());
    expect(backend.saveSettings.mock.calls[0][0].maxItemBytes).toBe(
      limitMb * 1024 * 1024
    );
  });

  it.each([0, 501])('rejects %i MB without invoking the backend', async (limitMb) => {
    const { backend } = setup();
    await ready();

    await fireEvent.input(screen.getByLabelText('单次内容上限 (MB)'), {
      target: { value: String(limitMb) }
    });
    await fireEvent.click(screen.getByRole('button', { name: '保存设置' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('1–500 MB');
    expect(backend.saveSettings).not.toHaveBeenCalled();
  });

  it('restores the persisted hotkey when native registration fails', async () => {
    const { store } = setup({
      saveSettings: async () => {
        throw { code: 'platform', message: '快捷键已被其他应用占用' };
      }
    });
    await ready();

    const hotkey = screen.getByLabelText('全局热键');
    const theme = screen.getByLabelText('主题');
    await fireEvent.input(hotkey, { target: { value: 'Ctrl+Alt+X' } });
    await fireEvent.change(theme, { target: { value: 'dark' } });
    await fireEvent.click(screen.getByRole('button', { name: '保存设置' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      '快捷键已被其他应用占用'
    );
    expect(hotkey).toHaveValue('Ctrl+Shift+V');
    expect(theme).toHaveValue('dark');
    expect(get(store).value?.hotkey).toBe('Ctrl+Shift+V');
  });

  it('saves theme, motion, autostart, and clear-on-exit controls', async () => {
    const { backend } = setup();
    await ready();

    await fireEvent.change(screen.getByLabelText('主题'), {
      target: { value: 'dark' }
    });
    await fireEvent.input(screen.getByLabelText('动画强度'), {
      target: { value: '0.6' }
    });
    await fireEvent.click(screen.getByLabelText('开机启动'));
    await fireEvent.click(screen.getByLabelText('退出时清空普通历史'));
    await fireEvent.click(screen.getByRole('button', { name: '保存设置' }));

    await waitFor(() => expect(backend.saveSettings).toHaveBeenCalledOnce());
    expect(backend.saveSettings).toHaveBeenCalledWith({
      ...settings,
      theme: 'dark',
      motionScale: 0.6,
      autostart: true,
      clearOnExit: true
    });
  });

  it('cancels or confirms clearing normal history and displays app information', async () => {
    const cancel = setup({ confirm: () => false });
    await ready();
    expect(screen.getByText('版本 0.1.0')).toBeInTheDocument();
    expect(
      screen.getByText(String.raw`C:\Users\Tester\AppData\Roaming\Easy Clipboard`)
    ).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: '清空普通历史' }));
    expect(cancel.clearHistoryAction).not.toHaveBeenCalled();

    document.body.innerHTML = '';
    const confirm = setup({ confirm: () => true });
    await ready();
    await fireEvent.click(screen.getByRole('button', { name: '清空普通历史' }));
    await waitFor(() => expect(confirm.clearHistoryAction).toHaveBeenCalledOnce());
  });
});

describe('App routing', () => {
  it('renders the settings view for the settings window URL', async () => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'get_settings') return settings;
      if (command === 'get_app_info') {
        return { version: '0.1.0', appDataDir: 'C:\\AppData\\Easy Clipboard' };
      }
      return undefined;
    });
    window.history.replaceState({}, '', '/?view=settings');

    render(App);

    expect(
      await screen.findByRole('heading', { name: '设置' })
    ).toBeInTheDocument();
    expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  });
});
