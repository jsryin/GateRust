<script lang="ts">
  import { Activity, Cable, FileCode2, LogOut, Network, PanelLeftClose, PanelLeftOpen, ShieldCheck } from '@lucide/svelte';

  export let active: string;
  export let onNavigate: (page: string) => void;
  export let onLogout: () => void;
  let open = false;

  const items = [
    { id: 'dashboard', label: '仪表盘', icon: Activity },
    { id: 'tunnel', label: '分组与隧道', icon: Cable },
    { id: 'proxy', label: '域名与 SSL', icon: ShieldCheck },
    { id: 'client', label: '客户端配置', icon: FileCode2 }
  ];
</script>

<button class="mobile-menu icon-button" aria-label="打开导航" onclick={() => (open = !open)}>
  {#if open}<PanelLeftClose size={20} />{:else}<PanelLeftOpen size={20} />{/if}
</button>
<aside class:open>
  <div class="sidebar-brand"><Network size={22} /><span>GateRust</span></div>
  <nav>
    {#each items as item}
      <button class:active={active === item.id} onclick={() => { onNavigate(item.id); open = false; }}>
        <item.icon size={18} /><span>{item.label}</span>
      </button>
    {/each}
  </nav>
  <button class="logout" onclick={onLogout}><LogOut size={18} /><span>退出登录</span></button>
</aside>
