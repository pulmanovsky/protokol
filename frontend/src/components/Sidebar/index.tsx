'use client';

import React, { useState, useMemo, useEffect, useCallback } from 'react';
import { Settings, Home, Trash2, Pencil, SearchIcon, X, Upload, FileText, CalendarDays, BookText, PanelLeft } from 'lucide-react';
import { useRouter, usePathname } from 'next/navigation';
import { useSidebar } from './SidebarProvider';
import type { CurrentMeeting } from '@/components/Sidebar/SidebarProvider';
import { confirmNative } from '@/lib/confirm';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import { toast } from '@/lib/notice';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useImportDialog } from '@/contexts/ImportDialogContext';
import { useConfig } from '@/contexts/ConfigContext';

import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"

import { MessageToast } from '../MessageToast';
import Logo from '../Logo';
import { RENAME_MEETING_EVENT, DELETE_MEETING_EVENT } from '@/lib/meetingEvents';
import { Input } from '../ui/input';
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from '../ui/input-group';
import { popupNativeMenu } from '@/lib/nativeMenu';

interface SidebarItem {
  id: string;
  title: string;
  type: 'folder' | 'file';
  children?: SidebarItem[];
}

const Sidebar: React.FC = () => {
  const router = useRouter();
  const pathname = usePathname();
  const {
    currentMeeting,
    setCurrentMeeting,
    sidebarItems,
    isCollapsed,
    toggleCollapse,
    handleRecordingToggle,
    searchTranscripts,
    searchResults,
    isSearching,
    meetings,
    setMeetings,
    serverAddress
  } = useSidebar();

  // Get recording state from RecordingStateContext (single source of truth)
  const { isRecording } = useRecordingState();
  const { openImportDialog } = useImportDialog();
  const { betaFeatures } = useConfig();
  const [expandedFolders, setExpandedFolders] = useState<Set<string>>(new Set(['meetings']));
  const [searchQuery, setSearchQuery] = useState<string>('');
  const [showModelSettings, setShowModelSettings] = useState(false);
  const [modelConfig, setModelConfig] = useState<ModelConfig>({
    provider: 'ollama',
    model: '',
    whisperModel: '',
    apiKey: null,
    ollamaEndpoint: null
  });
  const [transcriptModelConfig, setTranscriptModelConfig] = useState<TranscriptModelProps>({
    provider: 'localWhisper',
    model: 'large-v3',
  });
  const [settingsSaveSuccess, setSettingsSaveSuccess] = useState<boolean | null>(null);

  // State for edit modal
  const [editModalState, setEditModalState] = useState<{ isOpen: boolean; meetingId: string | null; currentTitle: string }>({
    isOpen: false,
    meetingId: null,
    currentTitle: ''
  });
  const [editingTitle, setEditingTitle] = useState<string>('');

  // Ensure 'meetings' folder is always expanded
  useEffect(() => {
    if (!expandedFolders.has('meetings')) {
      const newExpanded = new Set(expandedFolders);
      newExpanded.add('meetings');
      setExpandedFolders(newExpanded);
    }
  }, [expandedFolders]);

  // useEffect(() => {
  //   if (settingsSaveSuccess !== null) {
  //     const timer = setTimeout(() => {
  //       setSettingsSaveSuccess(null);
  //     }, 3000);
  //   }
  // }, [settingsSaveSuccess]);



  useEffect(() => {
    // Note: Don't set hardcoded defaults - let DB be the source of truth
    const fetchModelConfig = async () => {
      // Only make API call if serverAddress is loaded
      if (!serverAddress) {
        console.log('Waiting for server address to load before fetching model config');
        return;
      }

      try {
        const data = await invoke('api_get_model_config') as any;
        if (data && data.provider !== null) {
          setModelConfig(data);
        }
      } catch (error) {
        console.error('Failed to fetch model config:', error);
      }
    };

    fetchModelConfig();
  }, [serverAddress]);


  useEffect(() => {
    // Note: Don't set hardcoded defaults - let DB be the source of truth
    const fetchTranscriptSettings = async () => {
      // Only make API call if serverAddress is loaded
      if (!serverAddress) {
        console.log('Waiting for server address to load before fetching transcript settings');
        return;
      }

      try {
        const data = await invoke('api_get_transcript_config') as any;
        if (data && data.provider !== null) {
          setTranscriptModelConfig(data);
        }
      } catch (error) {
        console.error('Failed to fetch transcript settings:', error);
      }
    };
    fetchTranscriptSettings();
  }, [serverAddress]);

  // Listen for model config updates from other components
  useEffect(() => {
    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<ModelConfig>('model-config-updated', (event) => {
        console.log('Sidebar received model-config-updated event:', event.payload);
        setModelConfig(event.payload);
      });

      return unlisten;
    };

    let cleanup: (() => void) | undefined;
    setupListener().then(fn => cleanup = fn);

    return () => {
      cleanup?.();
    };
  }, []);



  // Handle model config save
  const handleSaveModelConfig = async (config: ModelConfig) => {
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey,
        ollamaEndpoint: config.ollamaEndpoint,
      });

      setModelConfig(config);
      console.log('Model config saved successfully');
      setSettingsSaveSuccess(true);

      // Emit event to sync other components
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      // Track settings change
      await Analytics.trackSettingsChanged('model_config', `${config.provider}_${config.model}`);
    } catch (error) {
      console.error('Error saving model config:', error);
      setSettingsSaveSuccess(false);
    }
  };

  const handleSaveTranscriptConfig = async (updatedConfig?: TranscriptModelProps) => {
    try {
      const configToSave = updatedConfig || transcriptModelConfig;
      const payload = {
        provider: configToSave.provider,
        model: configToSave.model,
        apiKey: configToSave.apiKey ?? null
      };
      console.log('Saving transcript config with payload:', payload);

      await invoke('api_save_transcript_config', {
        provider: payload.provider,
        model: payload.model,
        apiKey: payload.apiKey,
      });


      setSettingsSaveSuccess(true);

      // Track settings change
      const transcriptConfigToSave = updatedConfig || transcriptModelConfig;
      await Analytics.trackSettingsChanged('transcript_config', `${transcriptConfigToSave.provider}_${transcriptConfigToSave.model}`);
    } catch (error) {
      console.error('Failed to save transcript config:', error);
      setSettingsSaveSuccess(false);
    }
  };

  // Handle search input changes
  const handleSearchChange = useCallback(async (value: string) => {
    setSearchQuery(value);

    // If search query is empty, just return to normal view
    if (!value.trim()) return;

    // Search through transcripts
    await searchTranscripts(value);

    // Make sure the meetings folder is expanded when searching
    if (!expandedFolders.has('meetings')) {
      const newExpanded = new Set(expandedFolders);
      newExpanded.add('meetings');
      setExpandedFolders(newExpanded);
    }
  }, [expandedFolders, searchTranscripts]);

  // Combine search results with sidebar items
  const filteredSidebarItems = useMemo(() => {
    if (!searchQuery.trim()) return sidebarItems;

    // If we have search results, highlight matching meetings
    if (searchResults.length > 0) {
      // Get the IDs of meetings that matched in transcripts
      const matchedMeetingIds = new Set(searchResults.map(result => result.id));

      return sidebarItems
        .map(folder => {
          // Always include folders in the results
          if (folder.type === 'folder') {
            if (!folder.children) return folder;

            // Filter children based on search results or title match
            const filteredChildren = folder.children.filter(item => {
              // Include if the meeting ID is in our search results
              if (matchedMeetingIds.has(item.id)) return true;

              // Or if the title matches the search query
              return item.title.toLowerCase().includes(searchQuery.toLowerCase());
            });

            return {
              ...folder,
              children: filteredChildren
            };
          }

          // For non-folder items, check if they match the search
          return (matchedMeetingIds.has(folder.id) ||
            folder.title.toLowerCase().includes(searchQuery.toLowerCase()))
            ? folder : undefined;
        })
        .filter((item): item is SidebarItem => item !== undefined); // Type-safe filter
    } else {
      // Fall back to title-only filtering if no transcript results
      return sidebarItems
        .map(folder => {
          // Always include folders in the results
          if (folder.type === 'folder') {
            if (!folder.children) return folder;

            // Filter children based on search query
            const filteredChildren = folder.children.filter(item =>
              item.title.toLowerCase().includes(searchQuery.toLowerCase())
            );

            return {
              ...folder,
              children: filteredChildren
            };
          }

          // For non-folder items, check if they match the search
          return folder.title.toLowerCase().includes(searchQuery.toLowerCase()) ? folder : undefined;
        })
        .filter((item): item is SidebarItem => item !== undefined); // Type-safe filter
    }
  }, [sidebarItems, searchQuery, searchResults, expandedFolders]);


  const handleDelete = async (itemId: string) => {
    console.log('Deleting item:', itemId);
    const payload = {
      meetingId: itemId
    };

    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('api_delete_meeting', {
        meetingId: itemId,
      });
      console.log('Meeting deleted successfully');
      const updatedMeetings = meetings.filter((m: CurrentMeeting) => m.id !== itemId);
      setMeetings(updatedMeetings);

      // Track meeting deletion
      Analytics.trackMeetingDeleted(itemId);

      // Show success toast
      toast.success("Запис видалено", {
        description: "Запис, стенограму і підсумок видалено"
      });

      // If deleting the active meeting, navigate to home
      if (currentMeeting?.id === itemId) {
        setCurrentMeeting({ id: 'intro-call', title: 'Новий запис' });
        router.push('/');
      }
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      toast.error("Не вдалося видалити запис", {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const askDelete = async (itemId: string) => {
    if (await confirmNative('Видалити запис?', 'Звук, стенограму та документ буде видалено назавжди.', 'Видалити')) {
      handleDelete(itemId);
    }
  };

  // Handle modal editing of meeting names
  const handleEditStart = (meetingId: string, currentTitle: string) => {
    setEditModalState({
      isOpen: true,
      meetingId: meetingId,
      currentTitle: currentTitle
    });
    setEditingTitle(currentTitle);
  };

  const handleEditConfirm = async () => {
    const newTitle = editingTitle.trim();
    const meetingId = editModalState.meetingId;

    if (!meetingId) return;

    // Prevent empty titles
    if (!newTitle) {
      toast.error("Назва не може бути порожньою");
      return;
    }

    try {
      await invoke('api_save_meeting_title', {
        meetingId: meetingId,
        title: newTitle,
      });

      // Update local state
      const updatedMeetings = meetings.map((m: CurrentMeeting) =>
        m.id === meetingId ? { ...m, title: newTitle } : m
      );
      setMeetings(updatedMeetings);

      // Update current meeting if it's the one being edited
      if (currentMeeting?.id === meetingId) {
        setCurrentMeeting({ id: meetingId, title: newTitle });
      }

      // Track the edit
      Analytics.trackButtonClick('edit_meeting_title', 'sidebar');

      toast.success("Назву змінено");

      // Close modal and reset state
      setEditModalState({ isOpen: false, meetingId: null, currentTitle: '' });
      setEditingTitle('');
    } catch (error) {
      console.error('Failed to update meeting title:', error);
      toast.error("Не вдалося змінити назву", {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleEditCancel = () => {
    setEditModalState({ isOpen: false, meetingId: null, currentTitle: '' });
    setEditingTitle('');
  };

  const toggleFolder = (folderId: string) => {
    // Normal toggle behavior for all folders
    const newExpanded = new Set(expandedFolders);
    if (newExpanded.has(folderId)) {
      newExpanded.delete(folderId);
    } else {
      newExpanded.add(folderId);
    }
    setExpandedFolders(newExpanded);
  };

  // Expose setShowModelSettings to window for Rust tray to call
  useEffect(() => {
    (window as any).openSettings = () => {
      setShowModelSettings(true);
    };

    // Cleanup on unmount
    return () => {
      delete (window as any).openSettings;
    };
  }, []);

  const findMatchingSnippet = (itemId: string) => {
    if (!searchQuery.trim() || !searchResults.length) return null;
    return searchResults.find(result => result.id === itemId);
  };

  // The Протоколи screen asks the sidebar to open its rename / delete dialogs
  useEffect(() => {
    const onRename = (e: Event) => {
      const { id, title } = (e as CustomEvent<{ id: string; title: string }>).detail;
      handleEditStart(id, title);
    };
    const onDelete = (e: Event) => {
      const { id } = (e as CustomEvent<{ id: string }>).detail;
      askDelete(id);
    };
    window.addEventListener(RENAME_MEETING_EVENT, onRename);
    window.addEventListener(DELETE_MEETING_EVENT, onDelete);
    return () => {
      window.removeEventListener(RENAME_MEETING_EVENT, onRename);
      window.removeEventListener(DELETE_MEETING_EVENT, onDelete);
    };
  });

  const meetingItems: SidebarItem[] = filteredSidebarItems.find(item => item.id === 'meetings')?.children ?? [];
  const recentItems = meetingItems.slice(0, 3);
  // Under a recent meeting: «Затверджено» or how much of its transcript is checked
  const [reviewed, setReviewed] = useState<Record<string, number>>({});
  useEffect(() => {
    const load = () => invoke<Record<string, number>>('get_review_overview').then(setReviewed).catch(() => {});
    load();
    window.addEventListener('protokol:review-changed', load);
    return () => window.removeEventListener('protokol:review-changed', load);
  }, [meetings]);
  const statusOf = (id: string): { text: string; ok: boolean } | null => {
    const m = (meetings as CurrentMeeting[]).find((x) => x.id === id);
    if (m?.status === 'approved') return { text: 'Затверджено', ok: true };
    const r = reviewed[id];
    if (r === undefined || r <= 0) return null;
    return { text: r >= 0.999 ? 'Стенограму перевірено' : `Перевірено ${Math.round(r * 100)}%`, ok: r >= 0.999 };
  };
  const isSearchActive = searchQuery.trim().length > 0;

  // Exactly one item is highlighted, chosen by the current route
  const navItems = [
    { id: 'home', label: 'Сьогодні', icon: Home, href: '/', active: pathname === '/' },
    { id: 'protocols', label: 'Записи', icon: FileText, href: '/protocols', active: !!pathname?.startsWith('/protocols') || !!pathname?.startsWith('/meeting-details') },
    { id: 'calendar', label: 'Календар', icon: CalendarDays, href: '/calendar', active: !!pathname?.startsWith('/calendar') },
    { id: 'templates', label: 'Словник', icon: BookText, href: '/templates', active: !!pathname?.startsWith('/templates') },
  ].map(item => ({ ...item, onSelect: () => router.push(item.href) }));
  const isSettingsActive = !!pathname?.startsWith('/settings');

  const openMeeting = (item: SidebarItem) => {
    setCurrentMeeting({ id: item.id, title: item.title });
    router.push(`/meeting-details?id=${item.id}`);
  };

  const renderMeetingRow = (item: SidebarItem, withActions: boolean) => {
    const isActive = currentMeeting?.id === item.id && !!pathname?.includes('/meeting-details');
    const matchingResult = findMatchingSnippet(item.id);

    return (
      <div key={item.id} className="group">
        <div
          role="button"
          tabIndex={0}
          data-active={isActive}
          onClick={() => openMeeting(item)}
          onKeyDown={(e) => { if (e.key === 'Enter') openMeeting(item); }}
          onContextMenu={(e) => {
            if (!withActions) return;
            e.preventDefault();
            void popupNativeMenu([
              { text: 'Відкрити', action: () => openMeeting(item) },
              'separator',
              { text: 'Перейменувати', action: () => handleEditStart(item.id, item.title) },
              { text: 'Видалити…', action: () => askDelete(item.id) },
            ]);
          }}
          className={`nav-row ${!withActions && statusOf(item.id) ? 'h-11' : 'h-8'} px-2.5 text-[13px] cursor-pointer`}
        >
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="truncate">{item.title}</span>
            {!withActions && statusOf(item.id) && (
              <span className={`truncate text-[11px] ${statusOf(item.id)!.ok ? 'text-[#2fa66a]' : 'text-[color:var(--muted-text)]'}`}>{statusOf(item.id)!.text}</span>
            )}
          </span>
          {withActions && (
            <span className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
              <button
                type="button"
                onClick={(e) => { e.stopPropagation(); handleEditStart(item.id, item.title); }}
                className="p-1 rounded-md text-gray-500 hover:text-gray-900"
                aria-label="Перейменувати"
              >
                <Pencil className="w-3.5 h-3.5" />
              </button>
              <button
                type="button"
                onClick={(e) => { e.stopPropagation(); askDelete(item.id); }}
                className="p-1 rounded-md text-gray-500 hover:text-red-500"
                aria-label="Видалити"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
            </span>
          )}
        </div>
        {matchingResult && (
          <p className="mx-2.5 mb-1 text-xs text-gray-500 line-clamp-2">{matchingResult.matchContext}</p>
        )}
      </div>
    );
  };

  const renderCollapsed = () => (
    <TooltipProvider>
      <div className="flex flex-col items-center gap-1 px-3 pb-3 flex-1 min-h-0">
        <button type="button" onClick={toggleCollapse} className="icon-btn" aria-label="Розгорнути бічну панель">
          <PanelLeft className="w-[18px] h-[18px]" />
        </button>
        <div className="py-2"><Logo isCollapsed /></div>
        {navItems.map(({ id, label, icon: Icon, active, onSelect }) => (
          <Tooltip key={id}>
            <TooltipTrigger asChild>
              <button
                type="button"
                data-active={active}
                onClick={onSelect}
                className="icon-btn"
                aria-label={label}
              >
                <Icon className="w-[18px] h-[18px]" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right"><p>{label}</p></TooltipContent>
          </Tooltip>
        ))}
        <div className="mt-auto flex flex-col items-center gap-1">
          <Tooltip>
            <TooltipTrigger asChild>
              <button type="button" data-active={isSettingsActive} onClick={() => router.push('/settings')} className="icon-btn" aria-label="Налаштування">
                <Settings className="w-[18px] h-[18px]" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right"><p>Налаштування</p></TooltipContent>
          </Tooltip>
        </div>
      </div>
    </TooltipProvider>
  );

  const renderExpanded = () => (
    <div className="flex flex-col flex-1 min-h-0 px-3 pb-3">
      <div className="flex items-center h-8 px-1 mb-3">
        <Logo isCollapsed={false} />
      </div>

      <label className="sidebar-search no-drag flex items-center gap-2 h-8 px-2.5 mb-3">
        <SearchIcon className="w-3.5 h-3.5 text-gray-500 flex-shrink-0" />
        <input
          type="search"
          value={searchQuery}
          onChange={(e) => handleSearchChange(e.target.value)}
          placeholder="Пошук у записах"
          aria-label="Пошук у записах"
          className="flex-1 min-w-0 bg-transparent text-[13px] outline-none placeholder:text-gray-500"
        />
        {searchQuery && (
          <button type="button" onClick={() => handleSearchChange('')} className="text-gray-500 hover:text-gray-900" aria-label="Очистити пошук">
            <X className="w-3.5 h-3.5" />
          </button>
        )}
      </label>

      {!isSearchActive && (
        <nav aria-label="Навігація" className="flex flex-col gap-0.5">
          {navItems.map(({ id, label, icon: Icon, active, onSelect }) => (
            <button key={id} type="button" data-active={active} onClick={onSelect} className="nav-row h-8 px-2.5 gap-2.5 text-[15px]">
              <Icon className="w-4 h-4 flex-shrink-0" />
              <span className="truncate">{label}</span>
            </button>
          ))}
        </nav>
      )}

      <div className="flex flex-col flex-1 min-h-0 mt-4">
        {isSearchActive ? (
          <>
            <span className="sidebar-section">{isSearching ? 'Пошук…' : 'Результати пошуку'}</span>
            <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar flex flex-col gap-0.5">
              {meetingItems.length === 0 && !isSearching && (
                <p className="px-2.5 py-1 text-[13px] text-gray-500">Нічого не знайдено</p>
              )}
              {meetingItems.map(item => renderMeetingRow(item, false))}
            </div>
          </>
        ) : (
          recentItems.length > 0 && (
            <>
              <span className="sidebar-section">Нещодавні</span>
              <div className="flex flex-col gap-0.5">
                {recentItems.map(item => renderMeetingRow(item, false))}
              </div>
            </>
          )
        )}
      </div>

      <div className="flex flex-col pt-3">
        <button type="button" data-active={isSettingsActive} onClick={() => router.push('/settings')} className="nav-row h-8 px-2.5 gap-2.5 text-[15px]">
          <Settings className="w-4 h-4 flex-shrink-0" />
          <span>Налаштування</span>
        </button>
      </div>
    </div>
  );

  return (
    <aside
      className={`sidebar fixed top-2 left-2 bottom-2 z-40 flex flex-col transition-[width] duration-200 ${isCollapsed ? 'w-[74px]' : 'w-[212px]'}`}
    >
      {/* Clean strip for the overlaid traffic lights; nothing else may sit here */}
      <div className="titlebar h-[44px] flex-shrink-0 flex items-center justify-end px-2" data-tauri-drag-region>
        {!isCollapsed && (
          <button type="button" onClick={toggleCollapse} className="no-drag icon-btn-sm" aria-label="Згорнути бічну панель">
            <PanelLeft className="w-4 h-4" />
          </button>
        )}
      </div>

      {isCollapsed ? renderCollapsed() : renderExpanded()}

      {/* Edit Meeting Title Modal */}
      <Dialog open={editModalState.isOpen} onOpenChange={(open) => {
        if (!open) handleEditCancel();
      }}>
        <DialogContent className="sm:max-w-[425px]">
          <VisuallyHidden>
            <DialogTitle>Перейменувати запис</DialogTitle>
          </VisuallyHidden>
          <div className="py-4">
            <h3 className="text-lg font-semibold mb-4">Перейменувати запис</h3>
            <div className="space-y-4">
              <div>
                <label htmlFor="meeting-title" className="block text-sm font-medium text-gray-700 mb-2">
                  Назва
                </label>
                <input
                  id="meeting-title"
                  type="text"
                  value={editingTitle}
                  onChange={(e) => setEditingTitle(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      handleEditConfirm();
                    } else if (e.key === 'Escape') {
                      handleEditCancel();
                    }
                  }}
                  className="w-full px-3 h-10 rounded-[10px] bg-[var(--selected)] text-gray-900 focus:outline-none focus:ring-2 focus:ring-[hsl(var(--ring)/0.35)]"
                  placeholder="Назва запису"
                  autoFocus
                />
              </div>
            </div>
          </div>
          <DialogFooter>
            <button
              onClick={handleEditCancel}
              className="secondary-button h-10 px-4 text-sm font-semibold"
            >
              Скасувати
            </button>
            <button
              onClick={handleEditConfirm}
              className="accent-button h-10 px-4 text-sm font-semibold"
            >
              Зберегти
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </aside>
  );
};

export default Sidebar;
