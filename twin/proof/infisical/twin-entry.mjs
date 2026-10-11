// Infisical's backend as a request listener, for the Kern twin: its own start (src/main.ts), without what must not repeat in every
// copy of the app: telemetry instrumentation, removing its temporary folder, migrations (run once, by the proof's prepare step), signal
// handlers and listen(). The imports are its own build's files (tsup writes one .mjs per source file).
import { initializeHsmModule } from './dist/ee/services/hsm/hsm-fns.mjs';
import { hsmServiceFactory } from './dist/ee/services/hsm/hsm-service.mjs';
import { initDbConnection } from './dist/db/index.mjs';
import { keyValueStoreDALFactory } from './dist/keystore/key-value-store-dal.mjs';
import { keyStoreFactory } from './dist/keystore/keystore.mjs';
import { buildClickHouseFromConfig } from './dist/lib/config/clickhouse.mjs';
import { formatSmtpConfig, getDatabaseCredentials, getHsmConfig, initEnvConfig } from './dist/lib/config/env.mjs';
import { buildRedisFromConfig } from './dist/lib/config/redis.mjs';
import { initLogger } from './dist/lib/logger/index.mjs';
import { queueServiceFactory } from './dist/queue/index.mjs';
import { main } from './dist/server/app.mjs';
import { kmsRootConfigDALFactory } from './dist/services/kms/kms-root-config-dal.mjs';
import { smtpServiceFactory } from './dist/services/smtp/smtp-service.mjs';
import { superAdminDALFactory } from './dist/services/super-admin/super-admin-dal.mjs';

export async function createApp() {
  const logger = initLogger();
  const hsmConfig = getHsmConfig(logger);
  const hsmModule = initializeHsmModule(hsmConfig);
  hsmModule.initialize();
  const hsmService = hsmServiceFactory({ hsmModule: hsmModule.getModule(), envConfig: hsmConfig });
  await hsmService.startService();
  const credentials = getDatabaseCredentials(logger);
  const db = initDbConnection({ dbConnectionUri: credentials.dbConnectionUri, dbRootCert: credentials.dbRootCert, readReplicas: credentials.readReplicas });
  const superAdminDAL = superAdminDALFactory(db);
  const kmsRootConfigDAL = kmsRootConfigDALFactory(db);
  const envConfig = await initEnvConfig(hsmService, kmsRootConfigDAL, superAdminDAL, logger);
  const server = await main({
    db,
    superAdminDAL,
    kmsRootConfigDAL,
    hsmService,
    smtp: smtpServiceFactory(formatSmtpConfig()),
    logger,
    queue: queueServiceFactory(envConfig),
    keyStore: keyStoreFactory(envConfig, keyValueStoreDALFactory(db)),
    redis: buildRedisFromConfig(envConfig),
    clickhouse: buildClickHouseFromConfig(envConfig),
    envConfig,
  });
  return (req, res) => server.routing(req, res);
}
