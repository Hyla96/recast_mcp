import { useState, useEffect, useCallback } from 'react';

// ── Public types ───────────────────────────────────────────────────────────────

export interface UseDragToReorderOptions<T> {
  items: T[];
  onReorder: (reordered: T[]) => void;
}

export interface ItemDragProps {
  draggable: true;
  onDragStart: (e: React.DragEvent) => void;
  onDragOver: (e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  onDragEnd: () => void;
}

export interface DragHandleKeyProps {
  role: 'button';
  tabIndex: 0;
  'aria-label': string;
  onKeyDown: (e: React.KeyboardEvent) => void;
}

export interface UseDragToReorderResult<T> {
  /**
   * Returns drag-event props to spread onto the draggable row element.
   */
  getItemProps: (index: number) => ItemDragProps;
  /**
   * Returns keyboard + ARIA props to spread onto the drag handle element.
   */
  getDragHandleProps: (index: number) => DragHandleKeyProps;
  /**
   * Programmatically move an item from one index to another.
   */
  moveItem: (fromIndex: number, toIndex: number) => void;
  /**
   * Current announcement text for the aria-live region.
   */
  announcement: string;
  /**
   * True when the primary pointer is a coarse device (touchscreen).
   */
  isTouch: boolean;
  /**
   * Index of the item currently in keyboard-drag mode, or null.
   */
  keyboardDragIndex: number | null;
  /**
   * Index of the item being dragged via mouse DnD, or null.
   */
  isDraggingIndex: number | null;
  /**
   * Index of the current drop target during mouse DnD, or null.
   */
  dropTargetIndex: number | null;
}

// ── Hook ──────────────────────────────────────────────────────────────────────

/**
 * Generic drag-to-reorder hook using HTML5 Drag and Drop API.
 *
 * Also supports keyboard reorder: press Enter/Space on a drag handle to enter
 * keyboard-drag mode, then use Arrow Up/Down to move the item, and press
 * Enter/Space again (or Escape) to drop it.
 */
export function useDragToReorder<T>({
  items,
  onReorder,
}: UseDragToReorderOptions<T>): UseDragToReorderResult<T> {
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const [keyboardDragIndex, setKeyboardDragIndex] = useState<number | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const [isTouch, setIsTouch] = useState(false);

  // Detect coarse-pointer (touch) devices.
  useEffect(() => {
    const mq = window.matchMedia('(pointer: coarse)');
    setIsTouch(mq.matches);
    const handler = (e: MediaQueryListEvent) => setIsTouch(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  const moveItem = useCallback(
    (fromIndex: number, toIndex: number) => {
      if (fromIndex === toIndex) return;
      if (fromIndex < 0 || fromIndex >= items.length) return;
      if (toIndex < 0 || toIndex >= items.length) return;

      const next = [...items];
      const [moved] = next.splice(fromIndex, 1);
      if (moved === undefined) return;
      next.splice(toIndex, 0, moved);
      onReorder(next);
      setAnnouncement(`Field moved to position ${toIndex + 1} of ${items.length}`);
    },
    [items, onReorder]
  );

  // These factories are recreated each render; the hook is cheap enough that
  // this is fine. Consumers spread these onto their JSX elements directly.

  const getItemProps = (index: number): ItemDragProps => ({
    draggable: true,
    onDragStart: (e) => {
      setDragIndex(index);
      e.dataTransfer.effectAllowed = 'move';
    },
    onDragOver: (e) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'move';
      if (dropIndex !== index) setDropIndex(index);
    },
    onDrop: (e) => {
      e.preventDefault();
      if (dragIndex !== null && dragIndex !== index) {
        moveItem(dragIndex, index);
      }
      setDragIndex(null);
      setDropIndex(null);
    },
    onDragEnd: () => {
      setDragIndex(null);
      setDropIndex(null);
    },
  });

  const getDragHandleProps = (index: number): DragHandleKeyProps => ({
    role: 'button',
    tabIndex: 0,
    'aria-label': `Drag handle for field ${index + 1} of ${items.length}`,
    onKeyDown: (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        if (keyboardDragIndex === index) {
          // Drop
          setKeyboardDragIndex(null);
          setAnnouncement('Field dropped.');
        } else {
          // Grab
          setKeyboardDragIndex(index);
          setAnnouncement(
            `Grabbed field at position ${index + 1} of ${items.length}. ` +
              `Use Arrow Up and Down to move, then press Enter or Space to drop.`
          );
        }
      } else if (e.key === 'Escape') {
        if (keyboardDragIndex !== null) {
          setKeyboardDragIndex(null);
          setAnnouncement('Drag cancelled.');
        }
      } else if (keyboardDragIndex === index) {
        if (e.key === 'ArrowUp' && index > 0) {
          e.preventDefault();
          moveItem(index, index - 1);
          setKeyboardDragIndex(index - 1);
        } else if (e.key === 'ArrowDown' && index < items.length - 1) {
          e.preventDefault();
          moveItem(index, index + 1);
          setKeyboardDragIndex(index + 1);
        }
      }
    },
  });

  return {
    getItemProps,
    getDragHandleProps,
    moveItem,
    announcement,
    isTouch,
    keyboardDragIndex,
    isDraggingIndex: dragIndex,
    dropTargetIndex: dropIndex,
  };
}
