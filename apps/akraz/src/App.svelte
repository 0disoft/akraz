<script lang="ts">
  import { onMount } from 'svelte';

  import { daemonState } from './lib/state/daemonState.svelte';
  import { settingsState } from './lib/state/settingsState.svelte';
  import type { ControlMode, DaemonLifecyclePhase, PlatformCapabilities, ScreenEdge } from './lib/api/types';

  const modeLabels: Record<ControlMode, string> = {
    Local: '로컬',
    EnteringRemote: '넘어가는 중',
    Remote: '원격 제어 중',
    LeavingRemote: '돌아오는 중',
    Suspended: '일시정지',
  };

  const phaseLabels: Record<DaemonLifecyclePhase, string> = {
    not_running: '꺼져 있음',
    starting: '시작 중',
    running: '연결됨',
    unreachable: '응답 없음',
    failed: '확인 실패',
  };

  const capabilityRows: Array<{ key: keyof PlatformCapabilities; label: string }> = [
    { key: 'canCapturePointer', label: '마우스 잡기' },
    { key: 'canCaptureKeyboard', label: '키보드 잡기' },
    { key: 'canInjectPointer', label: '마우스 보내기' },
    { key: 'canInjectKeyboard', label: '키보드 보내기' },
  ];

  const edgeLabels: Record<ScreenEdge, string> = {
    left: '왼쪽',
    right: '오른쪽',
    top: '위',
    bottom: '아래',
  };

  const edgeOptions: ScreenEdge[] = ['left', 'right', 'top', 'bottom'];

  onMount(() => {
    void daemonState.refresh();
    void settingsState.load();
  });

  function canStartDaemon() {
    const phase = daemonState.snapshot?.phase;
    return phase === undefined || phase === 'not_running';
  }

  function canStopDaemon() {
    return daemonState.snapshot?.phase === 'running' && daemonState.snapshot.managedPid !== null;
  }

  function statusMessage() {
    if (daemonState.lastError) {
      return daemonState.lastError;
    }

    return daemonState.snapshot?.detail ?? '백그라운드 연결을 시작할 수 있어.';
  }

  function settingsMessage() {
    if (settingsState.lastError) {
      return settingsState.lastError;
    }
    if (settingsState.saved) {
      return '저장됨';
    }
    return '저장 전';
  }
</script>

<main class="shell">
  <section class="status-panel" aria-labelledby="home-title">
    <div class="title-row">
      <div>
        <p class="eyebrow">Akraz</p>
        <h1 id="home-title">입력 공유 상태</h1>
      </div>
      <div class="action-row" aria-label="데몬 제어">
        <button
          type="button"
          class="control-button secondary"
          disabled={daemonState.isBusy}
          onclick={() => daemonState.refresh()}
        >
          {daemonState.operation === 'refresh' ? '확인 중' : '새로고침'}
        </button>
        <button
          type="button"
          class="control-button"
          disabled={daemonState.isBusy || settingsState.isBusy || !canStartDaemon()}
          onclick={() => daemonState.start(settingsState.startOptions)}
        >
          {daemonState.operation === 'start' ? '시작 중' : '시작'}
        </button>
        <button
          type="button"
          class="control-button danger"
          disabled={daemonState.isBusy || !canStopDaemon()}
          onclick={() => daemonState.stop()}
        >
          {daemonState.operation === 'stop' ? '끄는 중' : '중지'}
        </button>
      </div>
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
          <p class="summary-label">
            {daemonState.snapshot ? phaseLabels[daemonState.snapshot.phase] : '확인 전'}
          </p>
          <p class="summary-value">{statusMessage()}</p>
        </div>
      </div>
    {/if}

    <section class="section-block settings-block" aria-labelledby="settings-title">
      <div class="section-heading-row">
        <h2 id="settings-title">수동 연결</h2>
        <span class:error-text={settingsState.lastError}>{settingsMessage()}</span>
      </div>

      <label class="toggle-row">
        <input
          type="checkbox"
          checked={settingsState.settings.captureInput}
          onchange={(event) => settingsState.updateCaptureInput(event.currentTarget.checked)}
        />
        <span>입력 잡기</span>
      </label>

      <div class="edge-list" aria-label="화면 끝 연결">
        {#each settingsState.settings.edgeBindings as binding, index}
          <div class="edge-row">
            <label>
              <span>내 화면</span>
              <select
                value={binding.localEdge}
                onchange={(event) => settingsState.updateEdgeBinding(index, 'localEdge', event.currentTarget.value)}
              >
                {#each edgeOptions as edge}
                  <option value={edge}>{edgeLabels[edge]}</option>
                {/each}
              </select>
            </label>

            <label class="peer-field">
              <span>기기 ID</span>
              <input
                type="text"
                value={binding.peerId}
                placeholder="linux-laptop"
                spellcheck="false"
                onchange={(event) => settingsState.updateEdgeBinding(index, 'peerId', event.currentTarget.value)}
              />
            </label>

            <label>
              <span>상대 화면</span>
              <select
                value={binding.remoteEdge}
                onchange={(event) => settingsState.updateEdgeBinding(index, 'remoteEdge', event.currentTarget.value)}
              >
                {#each edgeOptions as edge}
                  <option value={edge}>{edgeLabels[edge]}</option>
                {/each}
              </select>
            </label>

            <button
              type="button"
              class="icon-button"
              aria-label="연결 삭제"
              onclick={() => settingsState.removeEdgeBinding(index)}
            >
              ×
            </button>
          </div>
        {/each}
      </div>

      <div class="settings-actions">
        <button
          type="button"
          class="control-button secondary"
          disabled={settingsState.isBusy}
          onclick={() => settingsState.addEdgeBinding()}
        >
          추가
        </button>
        <button
          type="button"
          class="control-button"
          disabled={settingsState.isBusy}
          onclick={() => settingsState.save()}
        >
          {settingsState.operation === 'save' ? '저장 중' : '저장'}
        </button>
      </div>
    </section>
  </section>
</main>
