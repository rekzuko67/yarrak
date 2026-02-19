# Güvenlik Denetimi

Bu projedeki kod incelendi. **Zararlı yazılım veya gizli davranış yok.**

- **main.rs (Rust):** Sadece Discord API (`canary.discord.com`) ve senin `webhook_url` adresine bağlanır. Token/şifre başka yere gönderilmez. Sadece `mfa.txt` okunur (çalışma dizininden).
