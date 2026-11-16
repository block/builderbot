/**
 * What the top bar's history buttons read from navigation, as rune state
 * their component test owns: whether there is an entry each way. Because the
 * fields are `$state`, a test can change them while the buttons are mounted
 * and the buttons follow, as they do in the app. A plain object behind the
 * same getters is read once and never updates.
 */
class HistoryWorld {
  canGoBack = $state(false);
  canGoForward = $state(false);

  reset(): void {
    this.canGoBack = false;
    this.canGoForward = false;
  }
}

export const world = new HistoryWorld();
