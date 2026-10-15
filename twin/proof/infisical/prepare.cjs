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
    await db.raw("DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'kern_reader') THEN CREATE ROLE kern_reader LOGIN PASSWORD 'kern_reader'; END IF; END $$;");
    await db.raw('GRANT CONNECT ON DATABASE infisical TO kern_reader');
    await db.raw('GRANT USAGE ON SCHEMA public TO kern_reader');
    await db.raw('GRANT SELECT ON ALL TABLES IN SCHEMA public TO kern_reader');
    await db.raw('ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO kern_reader');
    console.log('the database is migrated and seeded, and a read-only role is ready');
  } finally {
    await db.destroy();
  }
  process.exit(0);
})().catch((error) => {
  console.error(error);
  process.exit(1);
});
