// Readies Infisical's database before each run exactly as its e2e tests do (backend/e2e-test/vitest-environment-knex.ts): the same
// start pieces, then its migrations and its seed, from its TypeScript sources. Its compiled seed files cannot run under Node's ESM
// loader (its seed data imports a named export from the CommonJS argon2), so this runs under tsx, Infisical's own TypeScript runner,
// from its backend folder (the Dockerfile copies it there). It runs in the app's container before the app starts; the twin never does
// this.
const path = require('node:path');
const { initLogger } = require('./src/lib/logger');
const { getDatabaseCredentials, getHsmConfig, initEnvConfig } = require('./src/lib/config/env');
const { initDbConnection } = require('./src/db');
const { superAdminDALFactory } = require('./src/services/super-admin/super-admin-dal');
const { kmsRootConfigDALFactory } = require('./src/services/kms/kms-root-config-dal');
const { initializeHsmModule } = require('./src/ee/services/hsm/hsm-fns');
const { hsmServiceFactory } = require('./src/ee/services/hsm/hsm-service');

(async () => {
  const logger = initLogger();
  const credentials = getDatabaseCredentials(logger);
  const hsmConfig = getHsmConfig(logger);
  const db = initDbConnection({ dbConnectionUri: credentials.dbConnectionUri, dbRootCert: credentials.dbRootCert });
  const hsmModule = initializeHsmModule(hsmConfig);
  hsmModule.initialize();
  const hsmService = hsmServiceFactory({ hsmModule: hsmModule.getModule(), envConfig: hsmConfig });
  await hsmService.startService();
  await initEnvConfig(hsmService, kmsRootConfigDALFactory(db), superAdminDALFactory(db), logger);
  try {
    await db.migrate.latest({ directory: path.join(__dirname, 'src/db/migrations'), loadExtensions: ['.ts'], tableName: 'infisical_migrations' });
    await db.seed.run({ directory: path.join(__dirname, 'src/db/seeds'), loadExtensions: ['.ts'] });
    // A read-only role for the proof's pgReads run: it may read every table and nothing more. The twin also wraps each read in a
    // READ ONLY transaction, so a write is refused twice over; this is the role an operator would point pgReads at in production.
    await db.raw("DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'kern_reader') THEN CREATE ROLE kern_reader LOGIN PASSWORD 'kern_reader'; END IF; END $$;");
    await db.raw('GRANT CONNECT ON DATABASE infisical TO kern_reader');
    await db.raw('GRANT USAGE ON SCHEMA public TO kern_reader');
    await db.raw('GRANT SELECT ON ALL TABLES IN SCHEMA public TO kern_reader');
    await db.raw('ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO kern_reader');
    console.log('the database is migrated and seeded, and a read-only role is ready');
  } finally {
    await db.destroy();
  }
  // The logger's transport thread would keep this process alive.
  process.exit(0);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
