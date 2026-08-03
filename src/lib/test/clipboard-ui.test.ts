// @vitest-environment jsdom

import '@testing-library/jest-dom/vitest';

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within
} from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import AppShell from '../components/AppShell.svelte';
import ClipboardCard from '../components/ClipboardCard.svelte';
import ClipboardList from '../components/ClipboardList.svelte';
import ContextMenu from '../components/ContextMenu.svelte';
import FilterBar from '../components/FilterBar.svelte';
import SearchField from '../components/SearchField.svelte';
import { clampMenuPosition } from '../components/context-menu';
import { createHistoryStore, type HistoryApi } from '../stores/history';
import type { ClipboardItem, HistoryQuery } from '../types';

const { convertFileSrcMock } = vi.hoisted(() => ({
  convertFileSrcMock: vi.fn((path: string) => `http://asset.localhost/${encodeURIComponent(path)}`)
}));

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: convertFileSrcMock,
  invoke: vi.fn()
}));

afterEach(() => {
  cleanup();
  convertFileSrcMock.mockClear();
});

const textItem: ClipboardItem = {
  id: 'text-1',
  kind: 'text',
  payload: {
    type: 'text',
    plain: 'Hello CAFÉ',
    html: null,
    rtf: null
  },
  fingerprint: 'text-fingerprint',
  preview: 'Hello CAFÉ',
  byteSize: 11,
  isFavorite: false,
  createdAtMs: 1,
  updatedAtMs: 4
};

const imageItem: ClipboardItem = {
  id: 'image-1',
  kind: 'image',
  payload: {
    type: 'image',
    pngPath: String.raw`C:\history\image.png`,
    thumbnailPath: String.raw`C:\history\thumbnail.png`,
    width: 320,
    height: 180
  },
  fingerprint: 'image-fingerprint',
  preview: '会议截图',
  byteSize: 1_572_864,
  isFavorite: true,
  createdAtMs: 2,
  updatedAtMs: 3
};

const audioItem: ClipboardItem = {
  id: 'audio-1',
  kind: 'files',
  payload: {
    type: 'files',
    entries: [
      {
        path: String.raw`C:\Media\voice.flac`,
        name: 'voice.flac',
        extension: 'flac',
        sizeBytes: 2_097_152,
        mediaKind: 'audio',
        available: true
      }
    ]
  },
  fingerprint: 'audio-fingerprint',
  preview: 'voice.flac',
  byteSize: 2_097_152,
  isFavorite: false,
  createdAtMs: 3,
  updatedAtMs: 2
};

const videoItem: ClipboardItem = {
  id: 'video-1',
  kind: 'files',
  payload: {
    type: 'files',
    entries: [
      {
        path: String.raw`C:\Media\demo.webm`,
        name: 'demo.webm',
        extension: 'webm',
        sizeBytes: 3_145_728,
        mediaKind: 'video',
        available: true
      }
    ]
  },
  fingerprint: 'video-fingerprint',
  preview: 'demo.webm',
  byteSize: 3_145_728,
  isFavorite: false,
  createdAtMs: 4,
  updatedAtMs: 1
};

const unavailableFileItem: ClipboardItem = {
  id: 'missing-1',
  kind: 'files',
  payload: {
    type: 'files',
    entries: [
      {
        path: String.raw`C:\Missing\gone.txt`,
        name: 'gone.txt',
        extension: 'txt',
        sizeBytes: 512,
        mediaKind: 'other',
        available: false
      }
    ]
  },
  fingerprint: 'missing-fingerprint',
  preview: 'gone.txt',
  byteSize: 512,
  isFavorite: false,
  createdAtMs: 5,
  updatedAtMs: 0
};

const pendingFileItem: ClipboardItem = {
  ...unavailableFileItem,
  id: 'pending-1',
  payload: {
    type: 'files',
    entries: [
      {
        path: String.raw`C:\Pending\pending.txt`,
        name: 'pending.txt',
        extension: 'txt',
        sizeBytes: 512,
        mediaKind: 'other',
        available: true,
        availabilityPending: true
      }
    ]
  },
  fingerprint: 'pending-fingerprint',
  preview: 'pending.txt'
};

function historyBackend(items: ClipboardItem[]) {
  return {
    listHistory: vi.fn(async (_query: HistoryQuery) => items),
    pasteItem: vi.fn(async () => undefined),
    copyItem: vi.fn(async () => undefined),
    setFavorite: vi.fn(async () => undefined),
    deleteItem: vi.fn(async () => undefined),
    clearHistory: vi.fn(async () => undefined)
  } satisfies HistoryApi;
}

function noopCardProps(item: ClipboardItem) {
  return {
    item,
    selected: false,
    busy: false,
    onPaste: vi.fn(async () => undefined),
    onContextMenu: vi.fn()
  };
}

describe('ClipboardCard', () => {
  it('renders text inside an accessible selected button', () => {
    render(ClipboardCard, {
      ...noopCardProps(textItem),
      selected: true
    });

    const card = screen.getByRole('button', { name: /Hello CAFÉ/ });
    expect(card).toHaveAttribute('aria-selected', 'true');
    expect(card).toHaveTextContent('Hello CAFÉ');
  });

  it('shows a lazy image thumbnail, safe alt, dimensions, and formatted bytes', () => {
    render(ClipboardCard, noopCardProps(imageItem));

    const image = screen.getByRole('img', { name: '图片剪贴记录：会议截图' });
    expect(image).toHaveAttribute('loading', 'lazy');
    expect(image).toHaveAttribute(
      'src',
      `http://asset.localhost/${encodeURIComponent(
        imageItem.payload.type === 'image' ? imageItem.payload.thumbnailPath : ''
      )}`
    );
    expect(image).not.toHaveAttribute(
      'src',
      imageItem.payload.type === 'image' ? imageItem.payload.thumbnailPath : ''
    );
    expect(screen.getByText('320 × 180')).toBeInTheDocument();
    expect(screen.getByText('1.5 MB')).toBeInTheDocument();
  });

  it('hides a failed thumbnail and preserves a visible fallback', async () => {
    render(ClipboardCard, noopCardProps(imageItem));
    const image = screen.getByRole('img', { name: '图片剪贴记录：会议截图' });

    await fireEvent.error(image);

    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(screen.getByText('缩略图不可用')).toBeInTheDocument();
  });

  it('falls back without rendering an image for an empty thumbnail path', () => {
    const emptyThumbnail: ClipboardItem = {
      ...imageItem,
      id: 'image-empty',
      payload: {
        type: 'image',
        pngPath: String.raw`C:\history\image.png`,
        thumbnailPath: '',
        width: 320,
        height: 180
      }
    };

    render(ClipboardCard, noopCardProps(emptyThumbnail));

    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(screen.getByText('缩略图不可用')).toBeInTheDocument();
    expect(convertFileSrcMock).not.toHaveBeenCalled();
  });

  it('falls back when the native asset URL conversion fails', () => {
    convertFileSrcMock.mockImplementationOnce(() => {
      throw new Error('asset conversion failed');
    });

    render(ClipboardCard, noopCardProps(imageItem));

    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(screen.getByText('缩略图不可用')).toBeInTheDocument();
  });

  it('localizes audio and video file labels and exposes names, sizes, and paths', () => {
    render(ClipboardList, {
      items: [audioItem, videoItem],
      selectedId: null,
      busyId: null,
      loading: false,
      error: null,
      onPaste: vi.fn(),
      onSelect: vi.fn(),
      onContextMenu: vi.fn()
    });

    expect(screen.getByText('音频')).toBeInTheDocument();
    expect(screen.getByText('视频')).toBeInTheDocument();
    expect(screen.getByText('voice.flac')).toBeInTheDocument();
    expect(screen.getByText('2 MB')).toBeInTheDocument();
    expect(screen.getByText(String.raw`C:\Media\voice.flac`)).toBeInTheDocument();
    expect(screen.getByText('demo.webm')).toBeInTheDocument();
    expect(screen.getByText('3 MB')).toBeInTheDocument();
  });

  it('caps large file previews without changing the payload passed to paste', async () => {
    const largeFileItem: ClipboardItem = {
      ...audioItem,
      id: 'large-files',
      payload: {
        type: 'files',
        entries: Array.from({ length: 7 }, (_, index) => ({
          path: `C:\\Batch\\file-${index}.txt`,
          name: `file-${index}.txt`,
          extension: 'txt',
          sizeBytes: index + 1,
          mediaKind: 'other' as const,
          available: true
        }))
      },
      preview: '7 files'
    };
    const onPaste = vi.fn(async (_item: ClipboardItem) => undefined);
    render(ClipboardCard, {
      ...noopCardProps(largeFileItem),
      onPaste
    });

    expect(screen.getByText('file-0.txt')).toBeInTheDocument();
    expect(screen.getByText('file-3.txt')).toBeInTheDocument();
    expect(screen.queryByText('file-4.txt')).not.toBeInTheDocument();
    expect(screen.getByText('另有 3 项')).toBeInTheDocument();

    await fireEvent.click(screen.getByRole('button', { name: /file-0\.txt/ }));
    expect(onPaste).toHaveBeenCalledWith(largeFileItem);
    const pastedItem = onPaste.mock.calls.at(0)?.at(0);
    if (pastedItem?.payload.type !== 'files') {
      throw new Error('expected the complete files payload');
    }
    expect(pastedItem.payload.entries).toHaveLength(7);
  });

  it('keeps a large pending file card enabled when an unrendered entry awaits validation', async () => {
    const largePendingItem: ClipboardItem = {
      ...audioItem,
      id: 'large-pending-files',
      payload: {
        type: 'files',
        entries: Array.from({ length: 7 }, (_, index) => ({
          path: `C:\\Batch\\pending-${index}.txt`,
          name: `pending-${index}.txt`,
          extension: 'txt',
          sizeBytes: index + 1,
          mediaKind: 'other' as const,
          available: true,
          availabilityPending: index === 6
        }))
      },
      preview: '7 files'
    };
    const onPaste = vi.fn();
    render(ClipboardCard, {
      ...noopCardProps(largePendingItem),
      onPaste
    });

    const card = screen.getByRole('button', { name: /pending-0\.txt/ });
    expect(card).toBeEnabled();
    expect(screen.queryByText('pending-6.txt')).not.toBeInTheDocument();
    expect(screen.getByText('部分文件待校验')).toBeInTheDocument();

    await fireEvent.click(card);
    expect(onPaste).toHaveBeenCalledWith(largePendingItem);
  });

  it('disables paste for unavailable files while preserving the context-menu gesture', async () => {
    const onPaste = vi.fn();
    const onContextMenu = vi.fn();
    const unavailableAndPendingItem: ClipboardItem = {
      ...unavailableFileItem,
      payload: {
        type: 'files',
        entries:
          unavailableFileItem.payload.type === 'files'
            ? unavailableFileItem.payload.entries.map((entry) => ({
                ...entry,
                availabilityPending: true
              }))
            : []
      }
    };
    render(ClipboardCard, {
      ...noopCardProps(unavailableAndPendingItem),
      onPaste,
      onContextMenu
    });

    const card = screen.getByRole('button', { name: /gone\.txt/ });
    expect(card).toBeDisabled();
    expect(screen.getByText('原文件不可用')).toBeInTheDocument();
    expect(screen.queryByText('部分文件待校验')).not.toBeInTheDocument();

    await fireEvent.click(card);
    await fireEvent.contextMenu(card, { clientX: 40, clientY: 50 });
    expect(onPaste).not.toHaveBeenCalled();
    expect(onContextMenu).toHaveBeenCalledOnce();
  });
});

describe('SearchField and FilterBar', () => {
  it('exposes accessible names and forwards normal-window input', async () => {
    const onSearch = vi.fn();
    render(SearchField, { value: '', onSearch });

    const search = screen.getByRole('searchbox', { name: '搜索剪贴板历史' });
    await fireEvent.input(search, { target: { value: 'needle' } });
    expect(onSearch).toHaveBeenLastCalledWith('needle');

    cleanup();
    render(FilterBar, { value: 'all', onChange: vi.fn() });
    expect(
      screen.getByRole('group', { name: '剪贴板类型筛选' })
    ).toBeInTheDocument();
  });

  it('selecting a filter hides nonmatching cards through the history query', async () => {
    const backend = historyBackend([textItem, imageItem]);
    const store = createHistoryStore(backend);
    render(AppShell, {
      store,
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    await screen.findByText('Hello CAFÉ');

    await fireEvent.click(screen.getByRole('button', { name: '图片' }));

    await waitFor(() => {
      expect(screen.queryByText('Hello CAFÉ')).not.toBeInTheDocument();
      expect(screen.getByText('320 × 180')).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: '图片' })).toHaveAttribute(
      'aria-pressed',
      'true'
    );
  });
});

describe('ContextMenu', () => {
  it('exposes exactly four confirmed menu actions for unavailable files', async () => {
    const onCopy = vi.fn();
    const onFavorite = vi.fn();
    const onDelete = vi.fn();
    const onReveal = vi.fn();
    render(ContextMenu, {
      item: unavailableFileItem,
      position: { x: 20, y: 30 },
      busy: false,
      onClose: vi.fn(),
      onCopy,
      onFavorite,
      onDelete,
      onReveal
    });

    const menu = screen.getByRole('menu', { name: '剪贴记录操作' });
    const actions = within(menu).getAllByRole('menuitem');
    expect(actions).toHaveLength(4);
    expect(within(menu).getByRole('menuitem', { name: '仅复制' })).toBeEnabled();
    expect(within(menu).getByRole('menuitem', { name: '收藏' })).toBeEnabled();
    expect(within(menu).getByRole('menuitem', { name: '删除' })).toBeEnabled();
    expect(
      within(menu).getByRole('menuitem', { name: '打开文件位置' })
    ).toBeEnabled();

    await fireEvent.click(within(menu).getByRole('menuitem', { name: '仅复制' }));
    expect(onCopy).toHaveBeenCalledOnce();
    expect(onFavorite).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(onReveal).not.toHaveBeenCalled();
  });

  it('uses the cancel-favorite label for favorites and closes on outside click or Escape', async () => {
    const onClose = vi.fn();
    render(ContextMenu, {
      item: imageItem,
      position: { x: 20, y: 30 },
      busy: false,
      onClose,
      onCopy: vi.fn(),
      onFavorite: vi.fn(),
      onDelete: vi.fn(),
      onReveal: vi.fn()
    });

    expect(screen.getByRole('menuitem', { name: '取消收藏' })).toBeInTheDocument();
    await fireEvent.pointerDown(document.body);
    expect(onClose).toHaveBeenCalledOnce();
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('clamps menu coordinates inside every viewport edge', () => {
    expect(clampMenuPosition(790, 590, 200, 160, 800, 600)).toEqual({
      x: 592,
      y: 432
    });
    expect(clampMenuPosition(-10, -20, 200, 160, 800, 600)).toEqual({
      x: 8,
      y: 8
    });
  });
});

describe('ClipboardList', () => {
  it('renders at most 100 normal items plus 20 favorites without dropping late favorites', () => {
    const normals = Array.from({ length: 105 }, (_, index) => ({
      ...textItem,
      id: `normal-${index}`,
      preview: `normal ${index}`,
      payload: {
        type: 'text',
        plain: `normal ${index}`,
        html: null,
        rtf: null
      },
      isFavorite: false
    }) satisfies ClipboardItem);
    const favorites = Array.from({ length: 21 }, (_, index) => ({
      ...textItem,
      id: `favorite-${index}`,
      preview: `favorite ${index}`,
      payload: {
        type: 'text',
        plain: `favorite ${index}`,
        html: null,
        rtf: null
      },
      isFavorite: true
    }) satisfies ClipboardItem);

    render(ClipboardList, {
      items: [...normals, ...favorites],
      selectedId: null,
      busyId: null,
      loading: false,
      error: null,
      onPaste: vi.fn(),
      onSelect: vi.fn(),
      onContextMenu: vi.fn()
    });

    expect(screen.getAllByRole('button')).toHaveLength(120);
    expect(screen.getByText('favorite 19')).toBeInTheDocument();
    expect(screen.queryByText('favorite 20')).not.toBeInTheDocument();
    expect(screen.getByText('显示 120 / 126 条')).toBeInTheDocument();
  });

  it('announces loading, error, and empty states', () => {
    const props = {
      items: [],
      selectedId: null,
      busyId: null,
      onPaste: vi.fn(),
      onSelect: vi.fn(),
      onContextMenu: vi.fn()
    };
    const loading = render(ClipboardList, {
      ...props,
      loading: true,
      error: null
    });
    expect(screen.getByRole('status')).toHaveTextContent('正在加载剪贴板历史');

    loading.rerender({ loading: false, error: '数据库不可用' });
    expect(screen.getByRole('alert')).toHaveTextContent('数据库不可用');

    loading.rerender({ loading: false, error: null });
    expect(screen.getByRole('status')).toHaveTextContent('暂无剪贴板历史');
  });
});

describe('AppShell native bridge and actions', () => {
  it('fills every supported window width without exceeding the 720px layout cap', async () => {
    render(AppShell, {
      store: createHistoryStore(historyBackend([])),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    const shell = document.querySelector<HTMLElement>('.app-shell');
    expect(shell).not.toBeNull();

    const style = getComputedStyle(shell!);
    expect(style.width).toBe('100%');
    expect(style.maxWidth).toBe('720px');
    expect(style.boxSizing).toBe('border-box');

    const maxWidth = Number.parseFloat(style.maxWidth);
    for (const windowWidth of [360, 460, 720]) {
      expect(Math.min(windowWidth, maxWidth)).toBe(windowWidth);
    }
  });

  it('refreshes on mount and routes the five exact overlay events', async () => {
    const backend = historyBackend([textItem, imageItem]);
    const store = createHistoryStore(backend);
    const hideOverlayAction = vi.fn(async () => undefined);
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    const listenToEvent = vi.fn(
      async (event: string, handler: (event: { payload: unknown }) => void) => {
        handlers.set(event, handler);
        return vi.fn();
      }
    );
    render(AppShell, {
      store,
      listenToEvent,
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined),
      hideOverlayAction
    });

    await screen.findByText('Hello CAFÉ');
    await waitFor(() => expect(handlers.size).toBe(5));
    expect([...handlers.keys()]).toEqual([
      'search-input',
      'selection-move',
      'selection-paste',
      'overlay-hide',
      'history-changed'
    ]);
    expect(backend.listHistory).toHaveBeenCalledOnce();

    handlers.get('history-changed')?.({ payload: undefined });
    await waitFor(() => expect(backend.listHistory).toHaveBeenCalledTimes(2));

    handlers.get('selection-move')?.({ payload: { delta: 1 } });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /会议截图/ })).toHaveAttribute(
        'aria-selected',
        'true'
      )
    );
    handlers.get('selection-paste')?.({ payload: undefined });
    await waitFor(() => expect(backend.pasteItem).toHaveBeenCalledWith('image-1'));
    await waitFor(() => expect(hideOverlayAction).toHaveBeenCalledOnce());

    const selectedImage = screen.getByRole('button', { name: /会议截图/ });
    await fireEvent.contextMenu(selectedImage, { clientX: 30, clientY: 40 });
    expect(screen.getByRole('menu', { name: '剪贴记录操作' })).toBeInTheDocument();
    handlers.get('overlay-hide')?.({ payload: undefined });
    await waitFor(() => {
      expect(hideOverlayAction).toHaveBeenCalledTimes(2);
      expect(screen.queryByRole('menu')).not.toBeInTheDocument();
      expect(document.querySelector('[aria-selected="true"]')).not.toBeInTheDocument();
    });
    handlers.get('history-changed')?.({ payload: undefined });
    await waitFor(() => expect(backend.listHistory).toHaveBeenCalledTimes(3));

    handlers.get('search-input')?.({
      payload: { action: 'insert', text: '🙂A' }
    });
    await waitFor(() =>
      expect(screen.getByRole('searchbox')).toHaveValue('🙂A')
    );
    handlers.get('search-input')?.({ payload: { action: 'deleteBackward' } });
    await waitFor(() => expect(screen.getByRole('searchbox')).toHaveValue('🙂'));
  });

  it('cleans late async event listeners after unmount without leaking', async () => {
    const resolvers: Array<(unlisten: () => void) => void> = [];
    const unlisteners = Array.from({ length: 5 }, () => vi.fn());
    let callIndex = 0;
    const listenToEvent = vi.fn(
      () =>
        new Promise<() => void>((resolve) => {
          const index = callIndex++;
          resolvers.push(() => resolve(unlisteners[index]));
        })
    );
    const backend = historyBackend([]);
    const view = render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent,
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    await waitFor(() => expect(resolvers).toHaveLength(5));

    view.unmount();
    resolvers.forEach((resolve) => resolve(() => undefined));

    await waitFor(() => {
      for (const unlisten of unlisteners) {
        expect(unlisten).toHaveBeenCalledOnce();
      }
    });
  });

  it('shows listener failures and safely degrades without an unhandled rejection', async () => {
    render(AppShell, {
      store: createHistoryStore(historyBackend([])),
      listenToEvent: vi.fn(async () => {
        throw new Error('native bridge unavailable');
      }),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'native bridge unavailable'
    );
  });

  it('keeps unavailable copy, delete, favorite, and reveal actions wired while paste stays disabled', async () => {
    const backend = historyBackend([unavailableFileItem]);
    backend.copyItem.mockRejectedValueOnce(new Error('复制失败'));
    const revealFileAction = vi.fn(async () => undefined);
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction
    });
    const card = await screen.findByRole('button', { name: /gone\.txt/ });

    expect(card).toBeDisabled();
    await fireEvent.contextMenu(card, { clientX: 30, clientY: 40 });
    await fireEvent.click(screen.getByRole('menuitem', { name: '仅复制' }));
    await waitFor(() => expect(backend.copyItem).toHaveBeenCalledWith('missing-1'));
    expect(await screen.findByRole('alert')).toHaveTextContent('复制失败');
    expect(backend.pasteItem).not.toHaveBeenCalled();

    await fireEvent.contextMenu(card, { clientX: 30, clientY: 40 });
    await fireEvent.click(
      screen.getByRole('menuitem', { name: '打开文件位置' })
    );
    expect(revealFileAction).toHaveBeenCalledWith(String.raw`C:\Missing\gone.txt`);
  });

  it('rejects native paste for unavailable selections', async () => {
    const backend = historyBackend([unavailableFileItem]);
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(
        async (event: string, handler: (event: { payload: unknown }) => void) => {
          handlers.set(event, handler);
          return vi.fn();
        }
      ),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    await screen.findByRole('button', { name: /gone\.txt/ });
    await waitFor(() => expect(handlers.has('selection-paste')).toBe(true));

    handlers.get('selection-paste')?.({ payload: undefined });

    await Promise.resolve();
    expect(backend.pasteItem).not.toHaveBeenCalled();
  });

  it('refreshes file availability after a native history change', async () => {
    const availableFileItem: ClipboardItem = {
      ...unavailableFileItem,
      payload: {
        type: 'files',
        entries: unavailableFileItem.payload.type === 'files'
          ? unavailableFileItem.payload.entries.map((entry) => ({
              ...entry,
              available: true
            }))
          : []
      }
    };
    const backend = historyBackend([availableFileItem]);
    backend.listHistory
      .mockResolvedValueOnce([availableFileItem])
      .mockResolvedValue([unavailableFileItem]);
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(
        async (event: string, handler: (event: { payload: unknown }) => void) => {
          handlers.set(event, handler);
          return vi.fn();
        }
      ),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    expect(await screen.findByRole('button', { name: /gone\.txt/ })).toBeEnabled();
    await waitFor(() => expect(handlers.has('history-changed')).toBe(true));

    handlers.get('history-changed')?.({ payload: undefined });

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /gone\.txt/ })).toBeDisabled()
    );
    expect(screen.getByText('原文件不可用')).toBeInTheDocument();
  });

  it('deduplicates repeated native paste events while the selected item is busy', async () => {
    let resolvePaste!: () => void;
    const pendingPaste = new Promise<void>((resolve) => {
      resolvePaste = resolve;
    });
    const backend = historyBackend([textItem]);
    backend.pasteItem.mockImplementation(() => pendingPaste.then(() => undefined));
    const hideOverlayAction = vi.fn(async () => undefined);
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(
        async (event: string, handler: (event: { payload: unknown }) => void) => {
          handlers.set(event, handler);
          return vi.fn();
        }
      ),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined),
      hideOverlayAction
    });
    await screen.findByText('Hello CAFÉ');
    await waitFor(() => expect(handlers.has('selection-paste')).toBe(true));

    handlers.get('selection-paste')?.({ payload: undefined });
    handlers.get('selection-paste')?.({ payload: undefined });

    expect(backend.pasteItem).toHaveBeenCalledTimes(1);
    resolvePaste();
    await waitFor(() => expect(hideOverlayAction).toHaveBeenCalledOnce());
  });

  it('allows a pending file paste and hides after backend validation succeeds', async () => {
    const backend = historyBackend([pendingFileItem]);
    const hideOverlayAction = vi.fn(async () => undefined);
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined),
      hideOverlayAction
    });
    const card = await screen.findByRole('button', { name: /pending\.txt/ });
    expect(card).toBeEnabled();
    expect(screen.getByText('部分文件待校验')).toBeInTheDocument();

    await fireEvent.click(card);

    await waitFor(() => expect(backend.pasteItem).toHaveBeenCalledWith('pending-1'));
    await waitFor(() => expect(hideOverlayAction).toHaveBeenCalledOnce());
    expect(screen.getByText('已粘贴', { selector: '.status' })).toBeInTheDocument();
  });

  it('keeps a pending file visible and reports the backend validation failure', async () => {
    const backend = historyBackend([pendingFileItem]);
    backend.pasteItem.mockRejectedValueOnce(new Error('paste failed'));
    const hideOverlayAction = vi.fn(async () => undefined);
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined),
      hideOverlayAction
    });
    const card = await screen.findByRole('button', { name: /pending\.txt/ });
    expect(card).toBeEnabled();

    await fireEvent.click(card);

    expect(await screen.findByRole('alert')).toHaveTextContent('paste failed');
    expect(backend.pasteItem).toHaveBeenCalledWith('pending-1');
    expect(hideOverlayAction).not.toHaveBeenCalled();
  });

  it('shows hide failures after a successful paste without misreporting paste failure', async () => {
    const backend = historyBackend([textItem]);
    const hideOverlayAction = vi.fn(async () => {
      throw new Error('hide after paste failed');
    });
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined),
      hideOverlayAction
    });
    const card = await screen.findByRole('button', { name: /Hello CAF/ });

    await fireEvent.click(card);

    expect(await screen.findByRole('alert')).toHaveTextContent('hide after paste failed');
    expect(backend.pasteItem).toHaveBeenCalledOnce();
    expect(hideOverlayAction).toHaveBeenCalledOnce();
    expect(screen.getByText('已粘贴', { selector: '.status' })).toBeInTheDocument();
  });

  it('blocks native paste while the context menu is open and resumes after dismissal', async () => {
    const backend = historyBackend([textItem]);
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(
        async (event: string, handler: (event: { payload: unknown }) => void) => {
          handlers.set(event, handler);
          return vi.fn();
        }
      ),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    const card = await screen.findByRole('button', { name: /Hello CAF/ });
    await waitFor(() => expect(handlers.has('selection-paste')).toBe(true));

    await fireEvent.contextMenu(card, { clientX: 30, clientY: 40 });
    handlers.get('selection-paste')?.({ payload: undefined });
    await Promise.resolve();
    expect(backend.pasteItem).not.toHaveBeenCalled();

    await fireEvent.click(
      screen.getByRole('button', { name: /关闭剪贴记录菜单/ })
    );
    handlers.get('selection-paste')?.({ payload: undefined });
    await waitFor(() => expect(backend.pasteItem).toHaveBeenCalledOnce());
  });

  it('shows native overlay hide failures after clearing transient UI state', async () => {
    const backend = historyBackend([textItem]);
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(
        async (event: string, handler: (event: { payload: unknown }) => void) => {
          handlers.set(event, handler);
          return vi.fn();
        }
      ),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined),
      hideOverlayAction: vi.fn(async () => {
        throw new Error('native hide failed');
      })
    });
    const card = await screen.findByRole('button', { name: /Hello CAF/ });
    await waitFor(() => expect(handlers.has('overlay-hide')).toBe(true));
    await fireEvent.contextMenu(card, { clientX: 30, clientY: 40 });

    handlers.get('overlay-hide')?.({ payload: undefined });

    expect(await screen.findByRole('alert')).toHaveTextContent('native hide failed');
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(document.querySelector('[aria-selected="true"]')).not.toBeInTheDocument();
  });

  it('opens settings and prevents duplicate paste commands while an item is busy', async () => {
    let resolvePaste!: () => void;
    const pendingPaste = new Promise<void>((resolve) => {
      resolvePaste = resolve;
    });
    const backend = historyBackend([textItem]);
    backend.pasteItem.mockImplementation(() => pendingPaste.then(() => undefined));
    const openSettingsAction = vi.fn(async () => undefined);
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction,
      revealFileAction: vi.fn(async () => undefined)
    });
    const card = await screen.findByRole('button', { name: /Hello CAFÉ/ });

    await fireEvent.click(card);
    await fireEvent.click(card);
    expect(backend.pasteItem).toHaveBeenCalledTimes(1);

    resolvePaste();
    await fireEvent.click(screen.getByRole('button', { name: '打开设置' }));
    expect(openSettingsAction).toHaveBeenCalledOnce();
  });

  it('consumes the first outside card click through a dismiss layer', async () => {
    const backend = historyBackend([textItem, imageItem]);
    render(AppShell, {
      store: createHistoryStore(backend),
      listenToEvent: vi.fn(async () => vi.fn()),
      openSettingsAction: vi.fn(async () => undefined),
      revealFileAction: vi.fn(async () => undefined)
    });
    const card = await screen.findByRole('button', { name: /Hello CAFÉ/ });
    await fireEvent.contextMenu(card, { clientX: 30, clientY: 40 });
    expect(screen.getByRole('menu')).toBeInTheDocument();

    const dismissLayer = screen.getByRole('button', {
      name: '关闭剪贴记录菜单'
    });
    await fireEvent.pointerDown(dismissLayer);
    expect(screen.getByRole('menu')).toBeInTheDocument();
    expect(backend.pasteItem).not.toHaveBeenCalled();
    await fireEvent.click(dismissLayer);

    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(backend.pasteItem).not.toHaveBeenCalled();

    await fireEvent.click(card);
    expect(backend.pasteItem).toHaveBeenCalledTimes(1);
  });
});
