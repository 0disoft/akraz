<script lang="ts">
  import { onMount } from 'svelte';

  import { daemonState } from './lib/state/daemonState.svelte';
  import type { ControlMode, PlatformCapabilities } from './lib/api/types';

  const modeLabels: Record<ControlMode, string> = {
    Local: '로컬',
    EnteringRemote: '넘어가는 중',
    Remote: '원격 제어 중',
    LeavingRemote: '돌아오는 중',
    Suspended: '일시정지',
  };

  const capabilityRows: Array<{ key: keyof PlatformCapabilities; label: string }> = [
    { key: 'canCapturePointer', label: '마우스 잡기' },
    { key: 'canCaptureKeyboard', label: '키보드 잡기' },
    { key: 'canInjectPointer', label: '마우스 보내기' },
    { key: 'canInjectKeyboard', label: '키보드 보내기' },
  ];

  onMount(() => {
    void daemonState.refresh();
  });
</script>

<main class="shell">
  <section class="status-panel" aria-labelledby="home-title">
    <div class="title-row">
      <div>
        <p class="eyebrow">Akraz</p>
        <h1 id="home-title">입력 공유 상태</h1>
      </div>
      <button type="button" class="refresh-button" disabled={daemonState.isLoading} onclick={() => daemonState.refresh()}>
        {daemonState.isLoading ? '확인 중' : '새로고침'}
      </button>
    </div>

    {#if daemonState.status}
      <div class="status-summary" aria-live="polite">
        <span class="status-dot ok" aria-hidden="true"></span>
        <div>
          <p class="summary-label">데몬 연결됨</p>
          <p class="summary-value">
            {modeLabels[daemonState.status.mode]} · v{daemonState.status.daemonVersion} · protocol
            {daemonState.status.protocol.major}.{daemonState.status.protocol.minor}
          </p>
        </div>
      </div>

      <div class="grid">
        <section class="section-block" aria-labelledby="capabilities-title">
          <h2 id="capabilities-title">입출력 권한</h2>
          <div class="capability-list">
            {#each capabilityRows as row}
              <div class="capability-row">
                <span>{row.label}</span>
                <strong class:ok={daemonState.status.capabilities[row.key]}>
                  {daemonState.status.capabilities[row.key] ? '가능' : '필요'}
                </strong>
              </div>
            {/each}
          </div>
        </section>

        <section class="section-block" aria-labelledby="peers-title">
          <h2 id="peers-title">연결된 기기</h2>
          {#if daemonState.status.peers.length === 0}
            <p class="muted">아직 연결된 기기가 없어.</p>
          {:else}
            <ul class="peer-list">
              {#each daemonState.status.peers as peer}
                <li>
                  <span>{peer.displayName}</span>
                  <strong>{peer.connected ? '연결됨' : '대기 중'}</strong>
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </div>
    {:else}
      <div class="status-summary error" aria-live="polite">
        <span class="status-dot error" aria-hidden="true"></span>
        <div>
          <p class="summary-label">데몬 연결 안 됨</p>
          <p class="summary-value">{daemonState.lastError ?? 'akraz-daemon을 먼저 실행해야 해.'}</p>
        </div>
      </div>
    {/if}
  </section>
</main>
