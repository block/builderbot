export type MenuIconComponent = typeof import('lucide-svelte').MoreVertical;

export type MenuActionItem = {
  type: 'action';
  label: string;
  icon?: MenuIconComponent;
  disabled?: boolean;
  danger?: boolean;
  closeOnSelect?: boolean;
  onSelect: () => void | Promise<void>;
};

export type MenuSeparatorItem = {
  type: 'separator';
};

export type MenuSubmenuItem = {
  type: 'submenu';
  label: string;
  icon?: MenuIconComponent;
  disabled?: boolean;
  children: MenuItem[];
};

export type MenuItem = MenuActionItem | MenuSeparatorItem | MenuSubmenuItem;
