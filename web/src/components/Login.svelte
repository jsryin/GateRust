<script lang="ts">
  import { LockKeyhole, LogIn } from '@lucide/svelte';
  import { login } from '../lib/api';

  export let onAuthenticated: (token: string) => void;
  let username = 'admin';
  let password = '';
  let error = '';
  let busy = false;

  async function submit() {
    busy = true;
    error = '';
    try {
      const session = await login(username, password);
      onAuthenticated(session.token);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : '登录失败';
    } finally {
      busy = false;
    }
  }
</script>

<main class="login-shell">
  <section class="login-panel">
    <div class="brand-mark"><LockKeyhole size={22} /></div>
    <div class="login-heading">
      <p class="eyebrow">GATERUST</p>
      <h1>中心控制台</h1>
      <p>使用管理员凭据继续</p>
    </div>
    <form onsubmit={(event) => { event.preventDefault(); submit(); }}>
      <label>用户名<input bind:value={username} autocomplete="username" required /></label>
      <label>密码<input bind:value={password} type="password" autocomplete="current-password" required /></label>
      {#if error}<p class="form-error" role="alert">{error}</p>{/if}
      <button class="primary wide" disabled={busy} type="submit">
        <LogIn size={17} />{busy ? '正在验证' : '登录'}
      </button>
    </form>
  </section>
</main>
