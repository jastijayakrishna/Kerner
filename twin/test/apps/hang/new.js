// Never answers: a busy loop the ghost's time limit has to stop.
export function createApp() {
  return () => {
    for (;;) {
      // spin
    }
  };
}
