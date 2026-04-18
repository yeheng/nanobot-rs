import { onMounted, onUnmounted, ref } from 'vue';

const STORAGE_KEY = 'gasket_sidebar_width';
const MIN_WIDTH = 200;
const MAX_WIDTH = 480;
const DEFAULT_WIDTH = 280;

export function useResizableSidebar(collapsedRef: { value: boolean }) {
  const sidebarWidth = ref(DEFAULT_WIDTH);
  const isResizing = ref(false);

  let startX = 0;
  let startWidth = 0;

  const loadSavedWidth = () => {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved) {
      sidebarWidth.value = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, parseInt(saved, 10)));
    }
  };

  const onResizeStart = (e: MouseEvent) => {
    if (collapsedRef.value) return;
    isResizing.value = true;
    startX = e.clientX;
    startWidth = sidebarWidth.value;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  };

  const onResizeMove = (e: MouseEvent) => {
    if (!isResizing.value) return;
    const delta = e.clientX - startX;
    sidebarWidth.value = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, startWidth + delta));
  };

  const onResizeEnd = () => {
    if (!isResizing.value) return;
    isResizing.value = false;
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    localStorage.setItem(STORAGE_KEY, String(sidebarWidth.value));
  };

  onMounted(() => {
    loadSavedWidth();
    window.addEventListener('mousemove', onResizeMove);
    window.addEventListener('mouseup', onResizeEnd);
  });

  onUnmounted(() => {
    window.removeEventListener('mousemove', onResizeMove);
    window.removeEventListener('mouseup', onResizeEnd);
  });

  return {
    sidebarWidth,
    isResizing,
    onResizeStart,
  };
}
