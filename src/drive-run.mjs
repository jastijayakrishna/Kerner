process.on('unhandledRejection', (e) => console.error('kern: unhandled rejection: ' + String((e && e.stack) || e).slice(0, 2000)));
process.on('uncaughtException', (e) => console.error('kern: uncaught exception: ' + String((e && e.stack) || e).slice(0, 2000)));
setTimeout(() => { console.error('kern: the driver did not finish within 90 seconds'); process.exit(0); }, 90000).unref();
try {
  await import('/kern/driver.mjs');
} catch (e) {
  console.error('kern: the driver failed: ' + String((e && e.stack) || e).slice(0, 4000));
}
setTimeout(() => process.exit(0), 300);
