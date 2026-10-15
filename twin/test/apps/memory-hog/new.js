
export function createApp() {
  return () => {
    const hog = [];
    for (;;) hog.push(new Array(1e6).fill(1));
  };
}
