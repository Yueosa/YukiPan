-- 访客密钥门切片: 管理员签发的临时上传密钥 (文档第 4、5 章)。
-- 密钥三档 30m/1h/24h, 文件寿命 = 密钥寿命; 吊销 = 主动级联删该密钥全部指向
-- (走正常删除流水线), 过期由每小时 TTL 清扫自然覆盖。
-- 管理员从私有区分享豁免密钥, key_id 记 NULL, 保留固定 24h TTL。

CREATE TABLE guest_keys (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    code       TEXT NOT NULL UNIQUE,
    note       TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ
);

ALTER TABLE guest_refs
    ADD COLUMN key_id UUID REFERENCES guest_keys(id);

-- 按密钥查文件 (吊销级联/列表计数)。
CREATE INDEX guest_refs_key_id_idx ON guest_refs (key_id);
