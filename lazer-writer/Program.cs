// LazerWriter <job.json>, the job { realm, skin, set: [{ name, hash }], remove: [name], result }.
// The files are already in lazer's file store under their hashes. The result file gets { ok } or
// { ok: false, error }.

using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Nodes;
using Realms;

var job = JsonNode.Parse(File.ReadAllText(args[0]))!;
var resultPath = (string)job["result"]!;
try
{
    Run(job);
    File.WriteAllText(resultPath, JsonSerializer.Serialize(new { ok = true }));
}
catch (Exception e)
{
    File.WriteAllText(resultPath, JsonSerializer.Serialize(new { ok = false, error = e.Message }));
}

static void Run(JsonNode job)
{
    var id = Guid.Parse((string)job["skin"]!);
    var set = job["set"]!.AsArray().Select(s => (name: Standard((string)s!["name"]!), hash: (string)s["hash"]!)).ToList();
    var remove = job["remove"]!.AsArray().Select(n => Standard((string)n!)).ToHashSet(StringComparer.OrdinalIgnoreCase);
    foreach (var (name, hash) in set)
    {
        if (hash.Length != 64 || !hash.All(Uri.IsHexDigit))
            throw new Exception($"{name} has no proper hash.");
        if (name.Length == 0 || name.Split('/').Any(p => p is "" or "." or ".."))
            throw new Exception($"{name} isn't a name lazer allows.");
    }

    // dynamic: the schema is whatever lazer's database has, this knows only the parts it touches
    var realmPath = (string)job["realm"]!;
    var config = new RealmConfiguration(realmPath) { IsDynamic = true };
    using var realm = Realm.GetInstance(config);
    var skin = realm.DynamicApi.Find("Skin", id) ?? throw new Exception("lazer doesn't have this skin anymore.");
    if (skin.DynamicApi.Get<bool>("Protected"))
        throw new Exception("lazer's own skins can't be changed.");

    realm.Write(() =>
    {
        var files = skin.DynamicApi.GetList<IEmbeddedObject>("Files");
        IEmbeddedObject? Named(string name) => files.FirstOrDefault(f => string.Equals(Standard(f.DynamicApi.Get<string>("Filename")), name, StringComparison.OrdinalIgnoreCase));

        foreach (var name in remove)
            if (Named(name) is { } gone)
                files.Remove(gone);
        foreach (var (name, hash) in set)
        {
            var file = realm.DynamicApi.Find("File", hash) ?? realm.DynamicApi.CreateObject("File", hash);
            var usage = Named(name);
            if (usage == null)
            {
                usage = realm.DynamicApi.AddEmbeddedObjectToList(files);
                usage.DynamicApi.Set("Filename", name);
            }
            usage.DynamicApi.Set("File", RealmValue.Object(file));
        }

        // an exception here undoes the whole write: lazer can't load a skin with a name but no file
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var f in files)
        {
            var name = f.DynamicApi.Get<string>("Filename");
            if (f.DynamicApi.Get<IRealmObject?>("File") == null)
                throw new Exception($"{name} came out without a file, nothing was saved.");
            if (!seen.Add(Standard(name)))
                throw new Exception($"{name} came out twice, nothing was saved.");
        }
        foreach (var (name, hash) in set)
            if (Named(name)?.DynamicApi.Get<IRealmObject?>("File")?.DynamicApi.Get<string>("Hash") != hash)
                throw new Exception($"{name} didn't take, nothing was saved.");

        // lazer finds an existing skin on import by this hash, the way its RealmArchiveModelImporter.ComputeHash
        // makes it. Left stale, importing the original skin again would hand back this edited one
        if (set.Any(s => Hashable(s.name)) || remove.Any(Hashable))
        {
            var store = Path.Combine(Path.GetDirectoryName(realmPath)!, "files");
            using var hashable = new MemoryStream();
            // lazer sorts with its culture, which is the invariant one unless a language is set
            foreach (var f in files.Where(f => Hashable(f.DynamicApi.Get<string>("Filename"))).OrderBy(f => f.DynamicApi.Get<string>("Filename"), StringComparer.InvariantCulture))
            {
                var hash = f.DynamicApi.Get<IRealmObject>("File").DynamicApi.Get<string>("Hash");
                var path = Path.Combine(store, hash[..1], hash[..2], hash);
                if (File.Exists(path))
                    using (var stream = File.OpenRead(path))
                        stream.CopyTo(hashable);
            }
            if (hashable.Length > 0)
                skin.DynamicApi.Set("Hash", Convert.ToHexStringLower(SHA256.HashData(hashable.ToArray())));
        }
    });
}

// lazer keeps names with forward slashes
static string Standard(string name) => name.Replace('\\', '/').Trim('/');

// SkinImporter's HashableFileTypes
static bool Hashable(string name) => name.EndsWith(".ini", StringComparison.OrdinalIgnoreCase) || name.EndsWith(".json", StringComparison.OrdinalIgnoreCase);
